use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
#[cfg(windows)]
use context_contracts::EventEnvelopeV2;
use context_contracts::{LocalApiRequest, LocalApiResponse};
use context_local_ipc::{NamedPipeServer, read_frame, write_frame};
use context_platform_windows::{DirectoryWatcher, WatchCancellation, WatchOutcome};
#[cfg(windows)]
use time::OffsetDateTime;

use crate::{collector::CollectorState, handle_request};

#[cfg(windows)]
#[path = "personal.rs"]
mod personal;

pub fn run(root: PathBuf, database_path: PathBuf, max_batches: Option<usize>) -> Result<()> {
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
    event_loop(&mut state, &cancellation, events_rx, max_batches)
}

fn event_loop(
    state: &mut CollectorState,
    cancellation: &WatchCancellation,
    events: Receiver<RuntimeEvent>,
    max_batches: Option<usize>,
) -> Result<()> {
    let mut completed_batches = 0usize;
    let mut personal_events = 0usize;
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
                personal_events += 1;
            }
            RuntimeEvent::Api { request, response } => {
                let reply = handle_request(state.engine_mut(), request);
                let _ = response.send(reply);
            }
            RuntimeEvent::Fatal(error) => return Err(anyhow!(error)),
        }
    }
    println!(
        "agent stopped: watcher_batches={completed_batches}; personal_events={personal_events}; last_sequence={}",
        state.last_sequence()
    );
    Ok(())
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
    Api {
        request: LocalApiRequest,
        response: Sender<LocalApiResponse>,
    },
    Fatal(String),
}
