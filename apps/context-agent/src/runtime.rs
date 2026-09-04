use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use context_content_vault::ContentVault;
#[cfg(windows)]
use context_contracts::EventEnvelopeV2;
use context_contracts::{LocalApiRequest, LocalApiResponse};
use context_local_ipc::{NamedPipeServer, read_frame, write_frame};
#[cfg(windows)]
use context_platform_windows::{ClipboardSnapshot, clipboard_snapshot_if_changed};
use context_platform_windows::{DirectoryWatcher, WatchCancellation, WatchOutcome};
#[cfg(windows)]
use context_screenpipe_adapter::{
    ScreenpipeClient, ScreenpipeCursor, ScreenpipeError, ScreenpipeFrame,
};
#[cfg(windows)]
use time::OffsetDateTime;

use crate::{collector::CollectorState, handle_request};

#[cfg(any(windows, test))]
#[path = "clipboard_capture.rs"]
mod clipboard_capture;

#[cfg(windows)]
#[path = "personal.rs"]
mod personal;

#[cfg(any(windows, test))]
#[path = "screenpipe_capture.rs"]
mod screenpipe_capture;

#[cfg(windows)]
const MAX_CLIPBOARD_RAW_UTF16_BYTES: usize = 8 * 1024 * 1024;
#[cfg(windows)]
const SCREENPIPE_SOURCE: &str = "screenpipe.local";
#[cfg(windows)]
const PERSONAL_SCOPE: &str = "scope.personal";

pub fn run(root: PathBuf, database_path: PathBuf, max_batches: Option<usize>) -> Result<()> {
    let content_vault = open_content_vault(&database_path)?;
    let mut state = CollectorState::open(root, database_path)?;
    let startup = state.reconcile()?;
    #[cfg(windows)]
    let screenpipe = screenpipe_runtime_config(&mut state);
    let cancellation = WatchCancellation::new()?;
    let watcher = DirectoryWatcher::open(state.root(), true, 64 * 1024)
        .with_context(|| format!("watch directory {}", state.root().display()))?;
    let pipe_server =
        NamedPipeServer::bind_current_user().context("bind current-user named pipe")?;
    let (events_tx, events_rx) = mpsc::channel();

    spawn_watcher(watcher, cancellation.clone(), events_tx.clone());
    spawn_personal_activity(cancellation.clone(), events_tx.clone());
    spawn_clipboard(cancellation.clone(), events_tx.clone());
    #[cfg(windows)]
    if let Some((client, cursor)) = screenpipe {
        spawn_screenpipe(client, cursor, cancellation.clone(), events_tx.clone());
    }
    spawn_ipc(pipe_server, cancellation.clone(), events_tx);
    if max_batches.is_none() {
        let signal = cancellation.clone();
        ctrlc::set_handler(move || {
            let _ = signal.cancel();
        })
        .context("install agent cancellation handler")?;
    }

    println!(
        "agent ready: startup_observed={}; startup_inserted={}; startup_deleted={}; startup_issues={}; sequence={}",
        startup.observed,
        startup.inserted,
        startup.deleted,
        startup.issues,
        state.last_sequence()
    );
    event_loop(
        &mut state,
        content_vault.as_ref(),
        &cancellation,
        events_rx,
        max_batches,
    )
}

fn event_loop(
    state: &mut CollectorState,
    _content_vault: Option<&ContentVault>,
    cancellation: &WatchCancellation,
    events: Receiver<RuntimeEvent>,
    max_batches: Option<usize>,
) -> Result<()> {
    let mut completed_batches = 0usize;
    let personal_events = std::cell::Cell::new(0usize);
    let clipboard_events = std::cell::Cell::new(0usize);
    let screenpipe_events = std::cell::Cell::new(0usize);
    loop {
        if cancellation.is_cancelled()? {
            break;
        }
        let event = match events.recv_timeout(Duration::from_millis(250)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("all agent runtime producers stopped"));
            }
        };
        match event {
            RuntimeEvent::Watch(Ok(WatchOutcome::Cancelled)) => break,
            RuntimeEvent::Watch(Err(error)) => return Err(error).context("watcher thread failed"),
            RuntimeEvent::Watch(Ok(WatchOutcome::Batch(batch))) => {
                let gap = batch.gap_detected;
                let changes = batch.changes.len();
                let ingested = state.ingest_batch(batch)?;
                completed_batches += 1;
                if gap {
                    let _ = state.reconcile()?;
                }
                println!(
                    "agent watcher batch: number={completed_batches}; changes={changes}; ingested={ingested}; gap={gap}; sequence={}",
                    state.last_sequence()
                );
                if max_batches.is_some_and(|maximum| completed_batches >= maximum) {
                    cancellation.cancel()?;
                    break;
                }
            }
            #[cfg(windows)]
            RuntimeEvent::Personal(event) => {
                state.engine_mut().ingest_v2(&event)?;
                personal_events.set(personal_events.get() + 1);
            }
            #[cfg(windows)]
            RuntimeEvent::Clipboard(observation) => {
                let vault = _content_vault.expect("Windows runtime opens the content vault");
                if let Some(event) = clipboard_capture::event_from_snapshot(
                    vault,
                    observation.snapshot,
                    observation.observed_at,
                    MAX_CLIPBOARD_RAW_UTF16_BYTES,
                )? {
                    state.engine_mut().ingest_v2(&event)?;
                    clipboard_events.set(clipboard_events.get() + 1);
                }
            }
            #[cfg(windows)]
            RuntimeEvent::Screenpipe {
                observation,
                committed,
            } => {
                let vault = _content_vault.expect("Windows runtime opens the content vault");
                let event = screenpipe_capture::event_from_frame(
                    vault,
                    observation.frame,
                    observation.screenshot,
                    observation.observed_at,
                )?;
                state.engine_mut().ingest_v2(&event)?;
                screenpipe_events.set(screenpipe_events.get() + 1);
                let _ = committed.send(());
            }
            RuntimeEvent::Api { request, response } => {
                let reply = handle_request(state.engine_mut(), request);
                let _ = response.send(reply);
            }
            RuntimeEvent::Fatal(error) => return Err(anyhow!(error)),
        }
    }
    println!(
        "agent stopped: watcher_batches={completed_batches}; personal_events={}; clipboard_events={}; screenpipe_events={}; last_sequence={}",
        personal_events.get(),
        clipboard_events.get(),
        screenpipe_events.get(),
        state.last_sequence()
    );
    Ok(())
}

#[cfg(windows)]
fn open_content_vault(database_path: &Path) -> Result<Option<ContentVault>> {
    let data_root = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let vault_root = data_root.join("vault").join("blobs");
    Ok(Some(ContentVault::open(&vault_root).with_context(
        || format!("open content vault {}", vault_root.display()),
    )?))
}

#[cfg(not(windows))]
fn open_content_vault(_database_path: &Path) -> Result<Option<ContentVault>> {
    Ok(None)
}

#[cfg(windows)]
fn screenpipe_runtime_config(
    state: &mut CollectorState,
) -> Option<(ScreenpipeClient, Option<ScreenpipeCursor>)> {
    let api_key = std::env::var("SCREENPIPE_LOCAL_API_KEY")
        .or_else(|_| std::env::var("SCREENPIPE_API_KEY"))
        .ok()?;
    let base_url = std::env::var("SCREENPIPE_LOCAL_API_URL")
        .unwrap_or_else(|_| "http://localhost:3030".into());
    let client = match ScreenpipeClient::new(&base_url, api_key) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("screenpipe adapter disabled: {error}");
            return None;
        }
    };
    let cursor = match state
        .engine_mut()
        .repository()
        .latest_raw_event_envelope_for_source(SCREENPIPE_SOURCE, PERSONAL_SCOPE)
    {
        Ok(Some(json)) => match serde_json::from_str::<EventEnvelopeV2>(&json) {
            Ok(event) => match (event.source_sequence, event.occurred_at) {
                (Some(frame_id), Some(captured_at)) => Some(ScreenpipeCursor {
                    frame_id,
                    captured_at,
                }),
                _ => {
                    eprintln!("screenpipe adapter disabled: persisted cursor event is incomplete");
                    return None;
                }
            },
            Err(error) => {
                eprintln!(
                    "screenpipe adapter disabled: persisted cursor event is invalid: {error}"
                );
                return None;
            }
        },
        Ok(None) => None,
        Err(error) => {
            eprintln!("screenpipe adapter disabled: cannot read persisted cursor: {error}");
            return None;
        }
    };
    Some((client, cursor))
}

fn spawn_watcher(
    mut watcher: DirectoryWatcher,
    cancellation: WatchCancellation,
    events: Sender<RuntimeEvent>,
) {
    thread::spawn(move || {
        loop {
            let outcome = watcher.read_next(&cancellation);
            let stopped = matches!(outcome, Ok(WatchOutcome::Cancelled)) || outcome.is_err();
            if events.send(RuntimeEvent::Watch(outcome)).is_err() || stopped {
                break;
            }
        }
    });
}

#[cfg(windows)]
fn spawn_personal_activity(cancellation: WatchCancellation, events: Sender<RuntimeEvent>) {
    thread::spawn(move || {
        let mut sampler = personal::PersonalActivitySampler::new();
        loop {
            if cancellation.is_cancelled().unwrap_or(true) {
                break;
            }

            let poll = sampler.poll(OffsetDateTime::now_utc());
            for diagnostic in poll.diagnostics {
                eprintln!("personal activity collector: {diagnostic}");
            }
            for event in poll.events {
                if events
                    .send(RuntimeEvent::Personal(Box::new(event)))
                    .is_err()
                {
                    return;
                }
            }

            thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(not(windows))]
fn spawn_personal_activity(_cancellation: WatchCancellation, _events: Sender<RuntimeEvent>) {}

#[cfg(windows)]
fn spawn_clipboard(cancellation: WatchCancellation, events: Sender<RuntimeEvent>) {
    thread::spawn(move || {
        let mut last_sequence = None;
        let mut last_error: Option<String> = None;
        loop {
            if cancellation.is_cancelled().unwrap_or(true) {
                break;
            }

            match clipboard_snapshot_if_changed(last_sequence, MAX_CLIPBOARD_RAW_UTF16_BYTES) {
                Ok(Some(snapshot)) => {
                    if last_error.take().is_some() {
                        eprintln!("clipboard collector: sampling recovered");
                    }
                    last_sequence = Some(snapshot.sequence());
                    if !matches!(snapshot, ClipboardSnapshot::NonText { .. }) {
                        let observation = ClipboardObservation {
                            observed_at: OffsetDateTime::now_utc(),
                            snapshot,
                        };
                        if events
                            .send(RuntimeEvent::Clipboard(Box::new(observation)))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    let message = error.to_string();
                    if last_error.as_deref() != Some(message.as_str()) {
                        eprintln!("clipboard collector: sampling failed: {message}");
                        last_error = Some(message);
                    }
                }
            }

            thread::sleep(Duration::from_millis(250));
        }
    });
}

#[cfg(not(windows))]
fn spawn_clipboard(_cancellation: WatchCancellation, _events: Sender<RuntimeEvent>) {}

#[cfg(windows)]
fn spawn_screenpipe(
    client: ScreenpipeClient,
    mut cursor: Option<ScreenpipeCursor>,
    cancellation: WatchCancellation,
    events: Sender<RuntimeEvent>,
) {
    thread::spawn(move || {
        let mut last_error: Option<String> = None;
        loop {
            if cancellation.is_cancelled().unwrap_or(true) {
                break;
            }
            let now = OffsetDateTime::now_utc();
            let frames = match client.fetch_frames_since(cursor.as_ref(), now) {
                Ok(frames) => frames,
                Err(error) => {
                    report_screenpipe_error(&mut last_error, &error);
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };

            let mut batch_failed = false;
            for frame in frames {
                if cancellation.is_cancelled().unwrap_or(true) {
                    return;
                }
                let screenshot = match client.fetch_frame_png(frame.frame_id) {
                    Ok(Some(bytes)) => screenpipe_capture::ScreenpipeScreenshot::Png(bytes),
                    Ok(None) => screenpipe_capture::ScreenpipeScreenshot::NotFound,
                    Err(ScreenpipeError::ResponseTooLarge { .. }) => {
                        screenpipe_capture::ScreenpipeScreenshot::OmittedTooLarge
                    }
                    Err(error) => {
                        report_screenpipe_error(&mut last_error, &error);
                        batch_failed = true;
                        break;
                    }
                };
                let next_cursor = ScreenpipeCursor::from_frame(&frame);
                let observation = ScreenpipeObservation {
                    frame,
                    screenshot,
                    observed_at: OffsetDateTime::now_utc(),
                };
                let (committed_tx, committed_rx) = mpsc::channel();
                if events
                    .send(RuntimeEvent::Screenpipe {
                        observation: Box::new(observation),
                        committed: committed_tx,
                    })
                    .is_err()
                {
                    return;
                }
                if committed_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                    return;
                }
                cursor = Some(next_cursor);
                if last_error.take().is_some() {
                    eprintln!("screenpipe adapter: sampling recovered");
                }
            }
            if batch_failed {
                thread::sleep(Duration::from_secs(5));
            } else {
                thread::sleep(Duration::from_secs(2));
            }
        }
    });
}

#[cfg(windows)]
fn report_screenpipe_error(last_error: &mut Option<String>, error: &ScreenpipeError) {
    let message = error.to_string();
    if last_error.as_deref() != Some(message.as_str()) {
        eprintln!("screenpipe adapter: {message}");
        *last_error = Some(message);
    }
}

#[cfg(windows)]
struct ClipboardObservation {
    observed_at: OffsetDateTime,
    snapshot: ClipboardSnapshot,
}

#[cfg(windows)]
struct ScreenpipeObservation {
    frame: ScreenpipeFrame,
    screenshot: screenpipe_capture::ScreenpipeScreenshot,
    observed_at: OffsetDateTime,
}

fn spawn_ipc(
    first_server: NamedPipeServer,
    cancellation: WatchCancellation,
    events: Sender<RuntimeEvent>,
) {
    thread::spawn(move || {
        let mut next_server = Some(first_server);
        loop {
            if cancellation.is_cancelled().unwrap_or(true) {
                break;
            }
            let server = next_server
                .take()
                .expect("the local API server is rebound after every connection");
            let mut connection = match server.accept() {
                Ok(connection) => connection,
                Err(error) => {
                    let _ = events.send(RuntimeEvent::Fatal(format!(
                        "accept local API client: {error}"
                    )));
                    break;
                }
            };
            let request: LocalApiRequest = match read_frame(&mut connection) {
                Ok(request) => request,
                Err(_) => {
                    drop(connection);
                    next_server = Some(match NamedPipeServer::bind_current_user() {
                        Ok(server) => server,
                        Err(error) => {
                            let _ = events.send(RuntimeEvent::Fatal(format!(
                                "rebind local API pipe: {error}"
                            )));
                            break;
                        }
                    });
                    continue;
                }
            };
            let (response_tx, response_rx) = mpsc::channel();
            if events
                .send(RuntimeEvent::Api {
                    request,
                    response: response_tx,
                })
                .is_err()
            {
                break;
            }
            let response = match response_rx.recv() {
                Ok(response) => response,
                Err(_) => break,
            };
            let _ = write_frame(&mut connection, &response);
            drop(connection);
            if cancellation.is_cancelled().unwrap_or(true) {
                break;
            }
            next_server = Some(match NamedPipeServer::bind_current_user() {
                Ok(server) => server,
                Err(error) => {
                    let _ = events.send(RuntimeEvent::Fatal(format!(
                        "rebind local API pipe: {error}"
                    )));
                    break;
                }
            });
        }
    });
}

enum RuntimeEvent {
    Watch(std::io::Result<WatchOutcome>),
    #[cfg(windows)]
    Personal(Box<EventEnvelopeV2>),
    #[cfg(windows)]
    Clipboard(Box<ClipboardObservation>),
    #[cfg(windows)]
    Screenpipe {
        observation: Box<ScreenpipeObservation>,
        committed: Sender<()>,
    },
    Api {
        request: LocalApiRequest,
        response: Sender<LocalApiResponse>,
    },
    Fatal(String),
}
