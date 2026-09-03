use context_contracts::EventEnvelopeV2;
use context_core::{IngestOutcome, RawEventRepository};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::*;

const RAW_V2_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS raw_event_v2_metadata (
  event_id TEXT PRIMARY KEY REFERENCES raw_event(event_id) ON DELETE CASCADE,
  envelope_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  payload_version INTEGER NOT NULL,
  occurred_at TEXT,
  device_id TEXT,
  content_refs_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS raw_event_v2_type_time
ON raw_event_v2_metadata(event_type, event_id);
"#;

fn ensure_v2_schema(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(RAW_V2_SCHEMA)?;
    Ok(())
}

impl SqliteRepository {
    pub fn raw_v2_event_count(&self) -> Result<u64, StorageError> {
        ensure_v2_schema(&self.connection)?;
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM raw_event_v2_metadata",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn raw_event_envelope_json(&self, event_id: Uuid) -> Result<Option<String>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT envelope_json FROM raw_event WHERE event_id = ?1",
                [event_id.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }
}

impl RawEventRepository for SqliteRepository {
    fn append_raw_v2(&mut self, event: &EventEnvelopeV2) -> Result<IngestOutcome, Self::Error> {
        ensure_v2_schema(&self.connection)?;
        let tx = self.connection.transaction()?;
        let envelope_json = serde_json::to_string(event)?;
        let content_refs_json = serde_json::to_string(&event.content_refs)?;
        let occurred_at = event.occurred_at.map(format_time).transpose()?;

        let inserted = tx.execute(
            "INSERT OR IGNORE INTO raw_event (
                event_id, schema_version, source, source_sequence, observed_at,
                ingested_at, scope_id, correlation_id, sensitivity, envelope_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.event_id.to_string(),
                event.envelope_version,
                event.source.0.as_str(),
                event.source_sequence,
                format_time(event.observed_at)?,
                format_time(event.ingested_at)?,
                event.scope_id.0.as_str(),
                event.correlation_id.map(|value| value.to_string()),
                serde_json::to_string(&event.sensitivity)?,
                envelope_json,
            ],
        )?;

        tx.execute(
            "INSERT OR IGNORE INTO raw_event_v2_metadata (
                event_id, envelope_version, event_type, payload_version,
                occurred_at, device_id, content_refs_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.event_id.to_string(),
                event.envelope_version,
                event.event_type.as_str(),
                event.payload_version,
                occurred_at,
                event.device_id.as_deref(),
                content_refs_json,
            ],
        )?;

        if let Some(sequence) = event.source_sequence {
            let requires_reconciliation = event.event_type == "collector.gap";
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
                    event.source.0.as_str(),
                    event.scope_id.0.as_str(),
                    sequence,
                    requires_reconciliation,
                    format_time(event.ingested_at)?,
                ],
            )?;
        }

        tx.commit()?;
        Ok(if inserted == 0 {
            IngestOutcome::Duplicate
        } else {
            IngestOutcome::Inserted
        })
    }
}

#[cfg(test)]
mod tests {
    use context_contracts::EventEnvelopeV2;
    use context_core::ContextEngine;
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn opaque_v2_event_is_retained_in_the_existing_raw_timeline() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let occurred_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_120).unwrap();
        let mut event = EventEnvelopeV2::observed(
            "wechat.message",
            99,
            "wechat.ui-parser",
            "scope.personal",
            observed_at,
            json!({"text": "sample ships Monday", "unknown": {"v": 99}}),
            "wechat-parser-v0",
            "opaque storage fixture",
        );
        event.occurred_at = Some(occurred_at);
        event.source_sequence = Some(11);

        assert_eq!(
            engine.ingest_v2(&event).unwrap().outcome,
            IngestOutcome::Inserted
        );
        assert_eq!(
            engine.ingest_v2(&event).unwrap().outcome,
            IngestOutcome::Duplicate
        );
        assert_eq!(engine.repository().raw_event_count().unwrap(), 1);
        assert_eq!(engine.repository().raw_v2_event_count().unwrap(), 1);
        assert_eq!(
            engine
                .repository()
                .last_source_sequence("wechat.ui-parser", "scope.personal")
                .unwrap(),
            Some(11)
        );

        let stored: serde_json::Value = serde_json::from_str(
            &engine
                .repository()
                .raw_event_envelope_json(event.event_id)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stored["event_type"], "wechat.message");
        assert_eq!(stored["payload_version"], 99);
        assert_eq!(stored["payload"]["unknown"]["v"], 99);
        assert_eq!(stored["occurred_at"], "2023-11-14T22:13:20Z");
        assert_eq!(stored["observed_at"], "2023-11-14T22:15:20Z");
    }

    #[test]
    fn v2_sidecar_schema_is_created_without_bumping_legacy_database_version() {
        let repository = SqliteRepository::in_memory().unwrap();
        assert_eq!(repository.raw_v2_event_count().unwrap(), 0);
        let version: u32 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_DATABASE_VERSION);
    }
}
