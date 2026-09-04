use std::path::Path;

use context_contracts::{EventEnvelope, FileIdentity};
use context_core::{DownloadMatchCandidate, EventRepository, IngestOutcome, ProjectionCommand};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("timestamp formatting failed: {0}")]
    Timestamp(#[from] time::error::Format),
    #[error("timestamp parsing failed: {0}")]
    TimestampParse(#[from] time::error::Parse),
    #[error("invalid UUID stored in database: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("database schema version {actual} is newer than supported version {supported}")]
    UnsupportedDatabaseVersion { actual: u32, supported: u32 },
}

pub struct SqliteRepository {
    connection: Connection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveLocation {
    pub identity: FileIdentity,
    pub path: String,
}

impl SqliteRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, StorageError> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut connection)?;
        Ok(Self { connection })
    }

    pub fn raw_event_count(&self) -> Result<u64, StorageError> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM raw_event", [], |row| row.get(0))?)
    }

    pub fn last_source_sequence(
        &self,
        source: &str,
        scope_id: &str,
    ) -> Result<Option<u64>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT last_sequence FROM collector_checkpoint
                 WHERE source = ?1 AND scope_id = ?2",
                params![source, scope_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn collector_reconciliation_required(
        &self,
        source: &str,
        scope_id: &str,
    ) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT reconciliation_required FROM collector_checkpoint
                 WHERE source = ?1 AND scope_id = ?2",
                params![source, scope_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn mark_collector_reconciled(
        &mut self,
        source: &str,
        scope_id: &str,
        last_sequence: u64,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO collector_checkpoint (
               source, scope_id, last_sequence, reconciliation_required, updated_at
             ) VALUES (?1, ?2, ?3, 0, ?4)
             ON CONFLICT(source, scope_id) DO UPDATE SET
               last_sequence = MAX(last_sequence, excluded.last_sequence),
               reconciliation_required = 0,
               updated_at = excluded.updated_at",
            params![
                source,
                scope_id,
                last_sequence,
                format_time(OffsetDateTime::now_utc())?
            ],
        )?;
        Ok(())
    }

    pub fn active_locations_in_scope(
        &self,
        scope_id: &str,
    ) -> Result<Vec<ActiveLocation>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT a.identity_provider, a.identity_namespace, a.identity_opaque, l.path
             FROM location l
             JOIN artifact a ON a.artifact_id = l.artifact_id
             WHERE l.active = 1 AND l.scope_id = ?1",
        )?;
        let rows = statement.query_map([scope_id], |row| {
            Ok(ActiveLocation {
                identity: FileIdentity {
                    provider: row.get(0)?,
                    namespace: row.get(1)?,
                    opaque_id: row.get(2)?,
                },
                path: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn artifact_count(&self) -> Result<u64, StorageError> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM artifact", [], |row| row.get(0))?)
    }

    pub fn active_location_for(
        &self,
        identity: &FileIdentity,
    ) -> Result<Option<String>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT l.path
                 FROM location l
                 JOIN artifact a ON a.artifact_id = l.artifact_id
                 WHERE a.identity_provider = ?1
                   AND a.identity_namespace = ?2
                   AND a.identity_opaque = ?3
                   AND l.active = 1",
                params![identity.provider, identity.namespace, identity.opaque_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn observed_download_edge_count(&self) -> Result<u64, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM edge WHERE predicate = 'downloaded_from' AND status = 'observed'",
            [],
            |row| row.get(0),
        )?)
    }
}

impl EventRepository for SqliteRepository {
    type Error = StorageError;

    fn append_with_projection(
        &mut self,
        event: &EventEnvelope,
        commands: &[ProjectionCommand],
    ) -> Result<IngestOutcome, Self::Error> {
        let tx = self.connection.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO raw_event (
                event_id, schema_version, source, source_sequence, observed_at,
                ingested_at, scope_id, correlation_id, sensitivity, envelope_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.event_id.to_string(),
                event.schema_version,
                event.source.0,
                event.source_sequence,
                format_time(event.observed_at)?,
                format_time(event.ingested_at)?,
                event.scope_id.0,
                event.correlation_id.map(|value| value.to_string()),
                serde_json::to_string(&event.sensitivity)?,
                serde_json::to_string(event)?,
            ],
        )?;

        if let Some(sequence) = event.source_sequence {
            let requires_reconciliation = matches!(
                event.payload,
                context_contracts::EventPayload::CollectorGap { .. }
            );
            tx.execute(
                "INSERT INTO collector_checkpoint (
                    source, scope_id, last_sequence, reconciliation_required, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(source, scope_id) DO UPDATE SET
                   last_sequence = MAX(last_sequence, excluded.last_sequence),
                   reconciliation_required = MAX(
                     reconciliation_required, excluded.reconciliation_required
                   ),
                   updated_at = excluded.updated_at",
                params![
                    event.source.0,
                    event.scope_id.0,
                    sequence,
                    requires_reconciliation,
                    format_time(event.ingested_at)?,
                ],
            )?;
        }

        if inserted == 0 {
            tx.commit()?;
            return Ok(IngestOutcome::Duplicate);
        }

        for command in commands {
            apply_command(&tx, command)?;
        }
        tx.commit()?;
        Ok(IngestOutcome::Inserted)
    }

    fn find_download_match(
        &self,
        path: &str,
        tolerance_seconds: i64,
    ) -> Result<Option<DownloadMatchCandidate>, Self::Error> {
        type MatchRow = (
            String,
            String,
            String,
            String,
            Vec<u8>,
            String,
            String,
            String,
        );
        let row: Option<MatchRow> = self
            .connection
            .query_row(
                "SELECT pd.download_id, pd.url, a.identity_provider, a.identity_namespace,
                        a.identity_opaque, pd.source_event_id, l.source_event_id,
                        CASE WHEN pd.observed_at >= l.observed_at
                             THEN pd.observed_at ELSE l.observed_at END
                 FROM pending_download pd
                 JOIN location l ON l.path = pd.final_path COLLATE NOCASE AND l.active = 1
                 JOIN artifact a ON a.artifact_id = l.artifact_id
                 WHERE pd.final_path = ?1 COLLATE NOCASE AND pd.matched_at IS NULL
                 ORDER BY pd.observed_at DESC
                 LIMIT 1",
                [path],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            download_id,
            url,
            provider,
            namespace,
            opaque_id,
            download_event_id,
            file_event_id,
            latest_observed_at,
        )) = row
        else {
            return Ok(None);
        };

        let (download_time, file_time): (String, String) = self.connection.query_row(
            "SELECT pd.observed_at, l.observed_at
             FROM pending_download pd
             JOIN location l ON l.path = pd.final_path COLLATE NOCASE AND l.active = 1
             WHERE pd.download_id = ?1",
            [&download_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let download_time = OffsetDateTime::parse(&download_time, &Rfc3339)?;
        let file_time = OffsetDateTime::parse(&file_time, &Rfc3339)?;
        if (download_time - file_time).whole_seconds().abs() > tolerance_seconds {
            return Ok(None);
        }

        Ok(Some(DownloadMatchCandidate {
            download_id: Uuid::parse_str(&download_id)?,
            identity: FileIdentity {
                provider,
                namespace,
                opaque_id,
            },
            url,
            source_event_ids: vec![
                Uuid::parse_str(&download_event_id)?,
                Uuid::parse_str(&file_event_id)?,
            ],
            observed_at: OffsetDateTime::parse(&latest_observed_at, &Rfc3339)?,
        }))
    }
}

fn apply_command(tx: &Transaction<'_>, command: &ProjectionCommand) -> Result<(), StorageError> {
    match command {
        ProjectionCommand::UpsertTask {
            task_id,
            name,
            started_at,
        } => {
            tx.execute(
                "INSERT INTO task (task_id, name, started_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(task_id) DO UPDATE SET name = excluded.name",
                params![task_id.to_string(), name, format_time(*started_at)?],
            )?;
        }
        ProjectionCommand::UpsertArtifactLocation {
            identity,
            path,
            scope_id,
            observed_at,
            change,
            source_event_id,
        } => {
            if matches!(change, context_contracts::FileChange::Deleted) {
                if let Some(artifact_id) = find_artifact(tx, identity)? {
                    tx.execute(
                        "UPDATE location SET
                           active = 0,
                           observed_at = ?3,
                           last_change = ?4,
                           source_event_id = ?5
                         WHERE artifact_id = ?1 AND path = ?2",
                        params![
                            artifact_id.to_string(),
                            path,
                            format_time(*observed_at)?,
                            serde_json::to_string(change)?,
                            source_event_id.to_string(),
                        ],
                    )?;
                }
                return Ok(());
            }
            let artifact_id = get_or_create_artifact(tx, identity)?;
            tx.execute(
                "UPDATE location SET active = 0 WHERE artifact_id = ?1",
                [artifact_id.to_string()],
            )?;
            tx.execute(
                "INSERT INTO location (
                    artifact_id, path, scope_id, observed_at, active, last_change, source_event_id
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)
                 ON CONFLICT(artifact_id, path) DO UPDATE SET
                   scope_id = excluded.scope_id,
                   observed_at = excluded.observed_at,
                   active = 1,
                   last_change = excluded.last_change,
                   source_event_id = excluded.source_event_id",
                params![
                    artifact_id.to_string(),
                    path,
                    scope_id,
                    format_time(*observed_at)?,
                    serde_json::to_string(change)?,
                    source_event_id.to_string(),
                ],
            )?;
        }
        ProjectionCommand::StorePendingDownload {
            download_id,
            url,
            referrer,
            final_path,
            observed_at,
            source_event_id,
        } => {
            tx.execute(
                "INSERT OR REPLACE INTO pending_download (
                    download_id, url, referrer, final_path, observed_at, source_event_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    download_id.to_string(),
                    url,
                    referrer,
                    final_path,
                    format_time(*observed_at)?,
                    source_event_id.to_string(),
                ],
            )?;
        }
        ProjectionCommand::AddObservedDownloadEdge {
            download_id,
            identity,
            url,
            source_event_ids,
            observed_at,
        } => {
            let artifact_id = get_or_create_artifact(tx, identity)?;
            tx.execute(
                "INSERT OR IGNORE INTO edge (
                    artifact_id, predicate, object_value, status, confidence,
                    evidence_json, created_at
                 ) VALUES (?1, 'downloaded_from', ?2, 'observed', 1.0, ?3, ?4)",
                params![
                    artifact_id.to_string(),
                    url,
                    serde_json::to_string(source_event_ids)?,
                    format_time(*observed_at)?,
                ],
            )?;
            tx.execute(
                "UPDATE pending_download SET matched_at = ?2 WHERE download_id = ?1",
                params![download_id.to_string(), format_time(*observed_at)?],
            )?;
        }
        ProjectionCommand::RecordCollectorGap {
            collector,
            last_sequence,
            reason,
            observed_at,
        } => {
            tx.execute(
                "INSERT INTO collector_gap (collector, last_sequence, reason, observed_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![collector, last_sequence, reason, format_time(*observed_at)?],
            )?;
        }
    }
    Ok(())
}

fn get_or_create_artifact(
    tx: &Transaction<'_>,
    identity: &FileIdentity,
) -> Result<Uuid, StorageError> {
    if let Some(existing) = find_artifact(tx, identity)? {
        return Ok(existing);
    }

    let artifact_id = Uuid::now_v7();
    tx.execute(
        "INSERT INTO artifact (
            artifact_id, identity_provider, identity_namespace, identity_opaque
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            artifact_id.to_string(),
            identity.provider,
            identity.namespace,
            identity.opaque_id,
        ],
    )?;
    Ok(artifact_id)
}

fn find_artifact(
    tx: &Transaction<'_>,
    identity: &FileIdentity,
) -> Result<Option<Uuid>, StorageError> {
    let existing = tx
        .query_row(
            "SELECT artifact_id FROM artifact
             WHERE identity_provider = ?1
               AND identity_namespace = ?2
               AND identity_opaque = ?3",
            params![identity.provider, identity.namespace, identity.opaque_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    existing
        .map(|value| Uuid::parse_str(&value).map_err(StorageError::InvalidUuid))
        .transpose()
}

fn format_time(value: OffsetDateTime) -> Result<String, time::error::Format> {
    value.format(&Rfc3339)
}

const CURRENT_DATABASE_VERSION: u32 = 3;

fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > CURRENT_DATABASE_VERSION {
        return Err(StorageError::UnsupportedDatabaseVersion {
            actual: version,
            supported: CURRENT_DATABASE_VERSION,
        });
    }
    let mut version = version;
    if version == 0 {
        let tx = connection.transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
        version = 1;
    }
    if version == 1 {
        let tx = connection.transaction()?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        version = 2;
    }
    if version == 2 {
        let tx = connection.transaction()?;
        tx.execute_batch(SCHEMA_V3)?;
        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;
        tx.commit()?;
    }
    Ok(())
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS raw_event (
  event_id TEXT PRIMARY KEY,
  schema_version INTEGER NOT NULL,
  source TEXT NOT NULL,
  source_sequence INTEGER,
  observed_at TEXT NOT NULL,
  ingested_at TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  correlation_id TEXT,
  sensitivity TEXT NOT NULL,
  envelope_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task (
  task_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  started_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifact (
  artifact_id TEXT PRIMARY KEY,
  identity_provider TEXT NOT NULL,
  identity_namespace TEXT NOT NULL,
  identity_opaque BLOB NOT NULL,
  UNIQUE(identity_provider, identity_namespace, identity_opaque)
);

CREATE TABLE IF NOT EXISTS location (
  artifact_id TEXT NOT NULL REFERENCES artifact(artifact_id),
  path TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  active INTEGER NOT NULL CHECK(active IN (0, 1)),
  last_change TEXT NOT NULL,
  source_event_id TEXT NOT NULL REFERENCES raw_event(event_id),
  PRIMARY KEY(artifact_id, path)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_location_per_artifact
ON location(artifact_id) WHERE active = 1;

CREATE TABLE IF NOT EXISTS pending_download (
  download_id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  referrer TEXT,
  final_path TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  source_event_id TEXT NOT NULL REFERENCES raw_event(event_id),
  matched_at TEXT
);

CREATE TABLE IF NOT EXISTS edge (
  edge_id INTEGER PRIMARY KEY AUTOINCREMENT,
  artifact_id TEXT NOT NULL REFERENCES artifact(artifact_id),
  predicate TEXT NOT NULL,
  object_value TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('observed', 'inferred', 'confirmed', 'rejected')),
  confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
  evidence_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(artifact_id, predicate, object_value, status)
);

CREATE TABLE IF NOT EXISTS collector_gap (
  gap_id INTEGER PRIMARY KEY AUTOINCREMENT,
  collector TEXT NOT NULL,
  last_sequence INTEGER,
  reason TEXT NOT NULL,
  observed_at TEXT NOT NULL
);
"#;

const SCHEMA_V2: &str = r#"
ALTER TABLE location ADD COLUMN scope_id TEXT NOT NULL DEFAULT '';

UPDATE location
SET scope_id = COALESCE(
  (SELECT raw_event.scope_id
   FROM raw_event
   WHERE raw_event.event_id = location.source_event_id),
  ''
);

CREATE TABLE collector_checkpoint (
  source TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  last_sequence INTEGER NOT NULL,
  reconciliation_required INTEGER NOT NULL CHECK(reconciliation_required IN (0, 1)),
  updated_at TEXT NOT NULL,
  PRIMARY KEY(source, scope_id)
);
"#;

const SCHEMA_V3: &str = r#"
CREATE INDEX IF NOT EXISTS raw_event_scope_observed_cursor
ON raw_event(scope_id, observed_at DESC, event_id DESC);
"#;

#[cfg(test)]
mod tests {
    use context_contracts::{EventEnvelope, EventPayload, FileChange, FileIdentity};
    use context_core::{ContextEngine, EventRepository, IngestOutcome, project_event};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn identity() -> FileIdentity {
        FileIdentity {
            provider: "windows-file-id".into(),
            namespace: "volume-a".into(),
            opaque_id: vec![1, 2, 3, 4],
        }
    }

    fn observed(payload: EventPayload) -> EventEnvelope {
        EventEnvelope::observed(
            "test.collector",
            "scope.test",
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            payload,
            "test-collector",
            "fixture",
        )
    }

    #[test]
    fn duplicate_event_is_idempotent() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = observed(EventPayload::TaskStarted {
            task_id: Uuid::now_v7(),
            name: "Research".into(),
        });

        assert_eq!(
            engine.ingest(&event).unwrap().outcome,
            IngestOutcome::Inserted
        );
        assert_eq!(
            engine.ingest(&event).unwrap().outcome,
            IngestOutcome::Duplicate
        );
        assert_eq!(engine.repository().raw_event_count().unwrap(), 1);
    }

    #[test]
    fn rename_keeps_one_artifact_and_updates_active_location() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let file_identity = identity();

        let created = observed(EventPayload::FileObserved {
            identity: file_identity.clone(),
            path: r"C:\work\draft.txt".into(),
            change: FileChange::Created,
        });
        let renamed = observed(EventPayload::FileObserved {
            identity: file_identity.clone(),
            path: r"C:\work\final.txt".into(),
            change: FileChange::Renamed,
        });

        engine.ingest(&created).unwrap();
        engine.ingest(&renamed).unwrap();

        assert_eq!(engine.repository().artifact_count().unwrap(), 1);
        assert_eq!(
            engine
                .repository()
                .active_location_for(&file_identity)
                .unwrap(),
            Some(r"C:\work\final.txt".into())
        );
    }

    #[test]
    fn matched_download_creates_an_observed_edge() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let source_events = vec![Uuid::now_v7(), Uuid::now_v7()];
        let matched = observed(EventPayload::DownloadMatched {
            download_id: Uuid::now_v7(),
            identity: identity(),
            url: "https://example.test/report.pdf".into(),
            source_event_ids: source_events,
        });

        engine.ingest(&matched).unwrap();

        assert_eq!(
            engine.repository().observed_download_edge_count().unwrap(),
            1
        );
    }

    #[test]
    fn exact_path_events_are_automatically_correlated_in_either_order() {
        for browser_first in [true, false] {
            let repository = SqliteRepository::in_memory().unwrap();
            let mut engine = ContextEngine::new(repository);
            let download = observed(EventPayload::BrowserDownloadObserved {
                download_id: Uuid::now_v7(),
                url: "https://example.test/report.pdf".into(),
                referrer: Some("https://example.test/".into()),
                final_path: r"c:\downloads\report.pdf".into(),
            });
            let file = observed(EventPayload::FileObserved {
                identity: identity(),
                path: r"C:\Downloads\REPORT.pdf".into(),
                change: FileChange::Created,
            });

            let reports = if browser_first {
                vec![
                    engine.ingest(&download).unwrap(),
                    engine.ingest(&file).unwrap(),
                ]
            } else {
                vec![
                    engine.ingest(&file).unwrap(),
                    engine.ingest(&download).unwrap(),
                ]
            };

            assert_eq!(
                reports
                    .iter()
                    .map(|report| report.derived_event_ids.len())
                    .sum::<usize>(),
                1
            );
            assert_eq!(
                engine.repository().observed_download_edge_count().unwrap(),
                1
            );
            assert_eq!(engine.repository().raw_event_count().unwrap(), 3);
        }
    }

    #[test]
    fn duplicate_replay_repairs_a_missing_derived_match() {
        let mut repository = SqliteRepository::in_memory().unwrap();
        let download = observed(EventPayload::BrowserDownloadObserved {
            download_id: Uuid::now_v7(),
            url: "https://example.test/recovery.pdf".into(),
            referrer: None,
            final_path: r"C:\downloads\recovery.pdf".into(),
        });
        let file = observed(EventPayload::FileObserved {
            identity: identity(),
            path: r"C:\downloads\recovery.pdf".into(),
            change: FileChange::Created,
        });

        repository
            .append_with_projection(&download, &project_event(&download))
            .unwrap();
        repository
            .append_with_projection(&file, &project_event(&file))
            .unwrap();
        assert_eq!(repository.observed_download_edge_count().unwrap(), 0);

        let mut engine = ContextEngine::new(repository);
        let repaired = engine.ingest(&file).unwrap();

        assert_eq!(repaired.outcome, IngestOutcome::Duplicate);
        assert_eq!(repaired.derived_event_ids.len(), 1);
        assert_eq!(
            engine.repository().observed_download_edge_count().unwrap(),
            1
        );
    }

    #[test]
    fn newer_database_version_is_rejected_instead_of_mutated() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();

        let error = SqliteRepository::from_connection(connection)
            .err()
            .expect("newer schema must be rejected");

        assert!(matches!(
            error,
            StorageError::UnsupportedDatabaseVersion {
                actual: 99,
                supported: CURRENT_DATABASE_VERSION
            }
        ));
    }

    #[test]
    fn events_outside_the_correlation_window_do_not_create_an_edge() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut download = observed(EventPayload::BrowserDownloadObserved {
            download_id: Uuid::now_v7(),
            url: "https://example.test/stale.pdf".into(),
            referrer: None,
            final_path: r"C:\downloads\stale.pdf".into(),
        });
        let mut file = observed(EventPayload::FileObserved {
            identity: identity(),
            path: r"C:\downloads\stale.pdf".into(),
            change: FileChange::Created,
        });
        download.observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        file.observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_181).unwrap();

        engine.ingest(&download).unwrap();
        let report = engine.ingest(&file).unwrap();

        assert!(report.derived_event_ids.is_empty());
        assert_eq!(
            engine.repository().observed_download_edge_count().unwrap(),
            0
        );
    }

    #[test]
    fn source_checkpoint_and_gap_state_are_committed_with_events() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut first = observed(EventPayload::TaskStarted {
            task_id: Uuid::now_v7(),
            name: "checkpoint".into(),
        });
        first.source_sequence = Some(7);
        engine.ingest(&first).unwrap();
        assert_eq!(
            engine
                .repository()
                .last_source_sequence("test.collector", "scope.test")
                .unwrap(),
            Some(7)
        );

        let mut gap = observed(EventPayload::CollectorGap {
            collector: "test.collector".into(),
            last_sequence: Some(7),
            reason: "fixture overflow".into(),
        });
        gap.source_sequence = Some(8);
        engine.ingest(&gap).unwrap();
        assert!(
            engine
                .repository()
                .collector_reconciliation_required("test.collector", "scope.test")
                .unwrap()
        );

        engine
            .repository_mut()
            .mark_collector_reconciled("test.collector", "scope.test", 8)
            .unwrap();
        assert!(
            !engine
                .repository()
                .collector_reconciliation_required("test.collector", "scope.test")
                .unwrap()
        );
    }

    #[test]
    fn deleted_file_deactivates_the_location_without_recreating_it() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let file_identity = identity();
        let created = observed(EventPayload::FileObserved {
            identity: file_identity.clone(),
            path: r"C:\work\deleted.txt".into(),
            change: FileChange::Created,
        });
        let deleted = observed(EventPayload::FileObserved {
            identity: file_identity.clone(),
            path: r"C:\work\deleted.txt".into(),
            change: FileChange::Deleted,
        });

        engine.ingest(&created).unwrap();
        engine.ingest(&deleted).unwrap();

        assert_eq!(
            engine
                .repository()
                .active_location_for(&file_identity)
                .unwrap(),
            None
        );
        assert!(
            engine
                .repository()
                .active_locations_in_scope("scope.test")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn version_one_database_is_migrated_forward() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection
            .execute(
                "INSERT INTO raw_event (
                   event_id, schema_version, source, observed_at, ingested_at,
                   scope_id, sensitivity, envelope_json
                 ) VALUES ('event-1', 1, 'fixture', '2026-09-01T00:00:00Z',
                           '2026-09-01T00:00:00Z', 'scope.migrated', 'metadata', '{}')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artifact (
                   artifact_id, identity_provider, identity_namespace, identity_opaque
                 ) VALUES ('artifact-1', 'fixture', 'volume', X'01')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO location (
                   artifact_id, path, observed_at, active, last_change, source_event_id
                 ) VALUES ('artifact-1', 'C:\\migrated.txt', '2026-09-01T00:00:00Z',
                           1, 'created', 'event-1')",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();

        let repository = SqliteRepository::from_connection(connection).unwrap();
        let version: u32 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();

        assert_eq!(version, CURRENT_DATABASE_VERSION);
        assert_eq!(
            repository
                .last_source_sequence("missing", "missing")
                .unwrap(),
            None
        );
        assert_eq!(
            repository
                .active_locations_in_scope("scope.migrated")
                .unwrap()
                .len(),
            1
        );
    }
}

mod retrieval;
mod v2;
