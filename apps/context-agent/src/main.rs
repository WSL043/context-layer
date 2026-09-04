use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use context_content_vault::ContentVault;
#[cfg(test)]
use context_contracts::EventEnvelopeV2;
use context_contracts::{EventEnvelope, EventPayload, FileChange};
use context_contracts::{
    LOCAL_API_VERSION, LocalApiCommand, LocalApiRequest, LocalApiResponse, LocalApiResult,
};
use context_core::{ContextEngine, IngestOutcome};
use context_local_ipc::{NamedPipeServer, read_frame, write_frame};
use context_platform_windows::{DirectoryAction, DirectoryWatcher, contract_path, file_identity};
use context_storage_sqlite::SqliteRepository;
use time::OffsetDateTime;
use uuid::Uuid;

mod collector;
mod content_read;
mod read_capability;
mod runtime;

fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        println!("context-agent foundation scaffold");
        println!("usage: context-agent --self-check [database-path]");
        println!("       context-agent --serve-once [database-path]");
        println!("       context-agent --watch-once <directory> [database-path]");
        println!("       context-agent --collect <directory> [database-path]");
        println!("       context-agent --collect-batches <directory> <count> [database-path]");
        println!("       context-agent --collector-health <directory> [database-path]");
        println!("       context-agent --run <directory> [database-path]");
        println!("       context-agent --run-batches <directory> <count> [database-path]");
        return Ok(());
    };
    if command == "--serve-once" {
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return serve_once(database_path);
    }
    if command == "--watch-once" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--watch-once requires a directory")?;
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return watch_once(root, database_path);
    }
    if command == "--collect" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--collect requires a directory")?;
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return collector::run(root, database_path, None);
    }
    if command == "--collect-batches" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--collect-batches requires a directory")?;
        let count = arguments
            .next()
            .context("--collect-batches requires a positive batch count")?
            .to_string_lossy()
            .parse::<usize>()
            .context("batch count must be a positive integer")?;
        if count == 0 {
            bail!("batch count must be greater than zero");
        }
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return collector::run(root, database_path, Some(count));
    }
    if command == "--collector-health" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--collector-health requires a directory")?
            .canonicalize()
            .context("resolve collector scope")?;
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        let repository = SqliteRepository::open(&database_path)
            .with_context(|| format!("open database {}", database_path.display()))?;
        let scope_id = contract_path(&root);
        println!(
            "collector-health: sequence={}; reconciliation_required={}; active_locations={}; raw_events={}; download_edges={}; scope={}",
            repository
                .last_source_sequence("windows.directory-watcher", &scope_id)?
                .unwrap_or(0),
            repository.collector_reconciliation_required("windows.directory-watcher", &scope_id)?,
            repository.active_locations_in_scope(&scope_id)?.len(),
            repository.raw_event_count()?,
            repository.observed_download_edge_count()?,
            root.display()
        );
        return Ok(());
    }
    if command == "--run" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--run requires a directory")?;
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return runtime::run(root, database_path, None);
    }
    if command == "--run-batches" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .context("--run-batches requires a directory")?;
        let count = arguments
            .next()
            .context("--run-batches requires a positive batch count")?
            .to_string_lossy()
            .parse::<usize>()
            .context("batch count must be a positive integer")?;
        if count == 0 {
            bail!("batch count must be greater than zero");
        }
        let database_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("context.db"));
        return runtime::run(root, database_path, Some(count));
    }
    if command != "--self-check" {
        bail!("unknown command: {}", command.to_string_lossy());
    }

    let database_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("context.db"));
    let repository = SqliteRepository::open(&database_path)
        .with_context(|| format!("open database {}", database_path.display()))?;
    let mut engine = ContextEngine::new(repository);
    let event = EventEnvelope::observed(
        "agent.self-check",
        "scope.self-check",
        OffsetDateTime::now_utc(),
        EventPayload::TaskStarted {
            task_id: Uuid::now_v7(),
            name: "Agent self-check".into(),
        },
        "context-agent",
        "local self-check",
    );
    let report = engine.ingest(&event)?;

    println!(
        "self-check: {:?}; raw_events={}; database={}",
        report.outcome,
        engine.repository().raw_event_count()?,
        database_path.display()
    );
    Ok(())
}

fn watch_once(root: PathBuf, database_path: PathBuf) -> Result<()> {
    let repository = SqliteRepository::open(&database_path)
        .with_context(|| format!("open database {}", database_path.display()))?;
    let mut engine = ContextEngine::new(repository);
    let mut watcher = DirectoryWatcher::open(&root, true, 64 * 1024)
        .with_context(|| format!("watch directory {}", root.display()))?;
    let batch = watcher
        .read_once()
        .context("read Windows directory changes")?;
    let observed_changes = batch.changes.len();
    let mut ingested = 0usize;

    if batch.gap_detected {
        let event = EventEnvelope::observed(
            "windows.directory-watcher",
            root.to_string_lossy(),
            OffsetDateTime::now_utc(),
            EventPayload::CollectorGap {
                collector: "windows.directory-watcher".into(),
                last_sequence: None,
                reason: "ReadDirectoryChangesW returned zero bytes; scope reconciliation required"
                    .into(),
            },
            "context-agent",
            "Windows watcher buffer overflow or enumeration gap",
        );
        engine.ingest(&event)?;
        ingested += 1;
    }

    for (index, change) in batch.changes.into_iter().enumerate() {
        let file_change = match change.action {
            DirectoryAction::Added => FileChange::Created,
            DirectoryAction::Modified => FileChange::Modified,
            DirectoryAction::RenamedTo => FileChange::Renamed,
            DirectoryAction::Removed
            | DirectoryAction::RenamedFrom
            | DirectoryAction::Unknown(_) => continue,
        };
        let path = root.join(change.relative_path);
        if !path.is_file() {
            continue;
        }
        let identity = match file_identity(&path) {
            Ok(identity) => identity,
            Err(_) => continue,
        };
        let mut event = EventEnvelope::observed(
            "windows.directory-watcher",
            root.to_string_lossy(),
            OffsetDateTime::now_utc(),
            EventPayload::FileObserved {
                identity,
                path: path.to_string_lossy().into_owned(),
                change: file_change,
            },
            "context-agent",
            "ReadDirectoryChangesW plus Windows file identity",
        );
        event.source_sequence = Some(index as u64);
        engine.ingest(&event)?;
        ingested += 1;
    }

    println!(
        "watch-once: changes={}; ingested={}; gap_detected={}; database={}",
        observed_changes,
        ingested,
        batch.gap_detected,
        database_path.display()
    );
    Ok(())
}

fn open_content_vault_for_database(database_path: &Path) -> Result<ContentVault> {
    let data_root = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let vault_root = data_root.join("vault").join("blobs");
    ContentVault::open(&vault_root)
        .with_context(|| format!("open content vault {}", vault_root.display()))
}

fn serve_once(database_path: PathBuf) -> Result<()> {
    let repository = SqliteRepository::open(&database_path)
        .with_context(|| format!("open database {}", database_path.display()))?;
    let mut engine = ContextEngine::new(repository);
    let content_vault = open_content_vault_for_database(&database_path)?;
    let server = NamedPipeServer::bind_current_user().context("bind current-user named pipe")?;
    let mut connection = server.accept().context("accept local API client")?;
    let request: LocalApiRequest = read_frame(&mut connection).context("read local API request")?;
    let response = handle_request(&mut engine, Some(&content_vault), request);
    write_frame(&mut connection, &response).context("write local API response")?;
    Ok(())
}

fn handle_request(
    engine: &mut ContextEngine<SqliteRepository>,
    content_vault: Option<&ContentVault>,
    request: LocalApiRequest,
) -> LocalApiResponse {
    let result = if request.protocol_version != LOCAL_API_VERSION {
        LocalApiResult::Error {
            code: "unsupported_protocol".into(),
            message: format!(
                "protocol version {} is unsupported; expected {}",
                request.protocol_version, LOCAL_API_VERSION
            ),
        }
    } else {
        match request.command {
            LocalApiCommand::Handshake { .. } => LocalApiResult::Ready {
                server_name: "context-agent".into(),
            },
            LocalApiCommand::SubmitEvent { event } => match engine.ingest(&event) {
                Ok(report) => LocalApiResult::EventAccepted {
                    event_id: event.event_id,
                    duplicate: report.outcome == IngestOutcome::Duplicate,
                },
                Err(error) => LocalApiResult::Error {
                    code: "ingest_failed".into(),
                    message: error.to_string(),
                },
            },
            LocalApiCommand::SubmitEventV2 { event } => match engine.ingest_v2(&event) {
                Ok(report) => LocalApiResult::EventAccepted {
                    event_id: event.event_id,
                    duplicate: report.outcome == IngestOutcome::Duplicate,
                },
                Err(error) => LocalApiResult::Error {
                    code: "ingest_failed".into(),
                    message: error.to_string(),
                },
            },
            LocalApiCommand::QueryTimeline {
                authorization,
                query,
            } => match read_capability::query_timeline_from_environment(
                engine.repository(),
                &authorization,
                query,
            ) {
                Ok(page) => LocalApiResult::TimelinePage { page },
                Err(error) => LocalApiResult::Error {
                    code: error.code().into(),
                    message: error.message(),
                },
            },
            LocalApiCommand::ReadTextContent {
                authorization,
                event_id,
                sha256,
            } => match content_vault {
                Some(vault) => match content_read::read_text_content_from_environment(
                    engine.repository(),
                    vault,
                    &authorization,
                    event_id,
                    &sha256,
                ) {
                    Ok(content) => LocalApiResult::TextContent { content },
                    Err(error) => LocalApiResult::Error {
                        code: error.code().into(),
                        message: error.message(),
                    },
                },
                None => LocalApiResult::Error {
                    code: "content_unavailable".into(),
                    message: "content vault is unavailable in this runtime".into(),
                },
            },
        }
    };
    LocalApiResponse {
        request_id: request.request_id,
        protocol_version: LOCAL_API_VERSION,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_v2_event_request_is_retained_and_acknowledged() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = EventEnvelopeV2::observed(
            "ui.window_focused",
            "windows.foreground",
            "scope.personal",
            OffsetDateTime::now_utc(),
            serde_json::json!({"process": "notepad.exe", "future": true}),
            "foreground-v0",
            "fixture",
        );
        let event_id = event.event_id;
        let request_id = Uuid::now_v7();
        let response = handle_request(
            &mut engine,
            None,
            LocalApiRequest {
                request_id,
                protocol_version: LOCAL_API_VERSION,
                command: LocalApiCommand::SubmitEventV2 {
                    event: Box::new(event),
                },
            },
        );

        assert_eq!(response.request_id, request_id);
        assert!(matches!(
            response.result,
            LocalApiResult::EventAccepted {
                event_id: accepted,
                duplicate: false
            } if accepted == event_id
        ));
        assert_eq!(engine.repository().raw_v2_event_count().unwrap(), 1);
    }

    #[test]
    fn submit_event_request_is_ingested_and_acknowledged() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = EventEnvelope::observed(
            "test.client",
            "scope.test",
            OffsetDateTime::now_utc(),
            EventPayload::TaskStarted {
                task_id: Uuid::now_v7(),
                name: "IPC test".into(),
            },
            "test",
            "local API fixture",
        );
        let event_id = event.event_id;
        let request_id = Uuid::now_v7();
        let response = handle_request(
            &mut engine,
            None,
            LocalApiRequest {
                request_id,
                protocol_version: LOCAL_API_VERSION,
                command: LocalApiCommand::SubmitEvent {
                    event: Box::new(event),
                },
            },
        );

        assert_eq!(response.request_id, request_id);
        assert!(matches!(
            response.result,
            LocalApiResult::EventAccepted {
                event_id: accepted,
                duplicate: false
            } if accepted == event_id
        ));
        assert_eq!(engine.repository().raw_event_count().unwrap(), 1);
    }
}
