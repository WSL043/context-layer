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
use time::OffsetDateTime;

use crate::{collector::CollectorState, handle_request};

#[cfg(any(windows, test))]
#[path = "clipboard_capture.rs"]
mod clipboard_capture;

#[cfg(windows)]
#[path = "personal.rs"]
mod personal;

#[cfg(windows)]
const MAX_CLIPBOARD_RAW_UTF16_BYTES: usize = 8 * 1024 * 1024;

pub fn run(root: PathBuf, database_path: PathBuf, max_batches: Option<usize>) -> Result<()> {
    let clipboard_vault = open_clipboard_vault(&database_path)?;
    let mut state = CollectorState::open(root, database_path)?;
    let startup = state.reconcile()?;
    let cancellation = WatchCancellation::new()?;
    let watcher = DirectoryWatcher::open(state.root(), true, 64 * 1024)
        .with_context(|| format!("watch directory {}", state.root().display()))?;
    let pipe_server =
        NamedPipeServer::bind_current_user().context("bind current-user named pipe")?;
    let (events_tx, events_rx) = mpsc::channel();

    spawn_watcher(watcher, cancellation.clone(), events_tx.clone());
    spawn_personal_activity(cancellation.clone(), events_tx.clone());
    spawn_clipboard(cancellation.clone(), events_tx.clone());
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
        clipboard_vault.as_ref(),
        &cancellation,
        events_rx,
        max_batches,
    )
}

fn event_loop(
    state: &mut CollectorState,
    _clipboard_vault: Option<&ContentVault>,
    cancellation: &WatchCancellation,
    events: Receiver<RuntimeEvent>,
    max_batches: Option<usize>,
) -> Result<()> {
    let mut completed_batches = 0usize;
    let personal_events = std::cell::Cell::new(0usize);
    let clipboard_events = std::cell::Cell::new(0usize);
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
                let vault = _clipboard_vault.expect("Windows runtime opens the clipboard vault");
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
            RuntimeEvent::Api { request, response } => {
                let reply = handle_request(state.engine_mut(), request);
                let _ = response.send(reply);
            }
            RuntimeEvent::Fatal(error) => return Err(anyhow!(error)),
        }
    }
    println!(
        "agent stopped: watcher_batches={completed_batches}; personal_events={}; clipboard_events={}; last_sequence={}",
        personal_events.get(),
        clipboard_events.get(),
        state.last_sequence()
    );
    Ok(())
}

#[cfg(windows)]
fn open_clipboard_vault(database_path: &Path) -> Result<Option<ContentVault>> {
    let data_root = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let vault_root = data_root.join("vault").join("blobs");
    Ok(Some(ContentVault::open(&vault_root).with_context(|| {
        format!("open content vault {}", vault_root.display())
    })?))
}

#[cfg(not(windows))]
fn open_clipboard_vault(_database_path: &Path) -> Result<Option<ContentVault>> {
    Ok(None)
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
struct ClipboardObservation {
    observed_at: OffsetDateTime,
    snapshot: ClipboardSnapshot,
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
    Api {
        request: LocalApiRequest,
        response: Sender<LocalApiResponse>,
    },
    Fatal(String),
}
