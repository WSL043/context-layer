use context_contracts::EventEnvelopeV2;
use context_core::{IngestOutcome, RawEventRepository};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::*;

impl SqliteRepository {
    pub fn raw_v2_event_count(&self) -> Result<u64, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM raw_event WHERE schema_version = 2",
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

    pub fn latest_raw_event_envelope_for_source(
        &self,
        source: &str,
        scope_id: &str,
    ) -> Result<Option<String>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT envelope_json
                 FROM raw_event
                 WHERE source = ?1 AND scope_id = ?2 AND source_sequence IS NOT NULL
                 ORDER BY source_sequence DESC
                 LIMIT 1",
                params![source, scope_id],
                |row| row.get(0),
            )
            .optional()?)
    }
}

impl RawEventRepository for SqliteRepository {
    fn append_raw_v2(&mut self, event: &EventEnvelopeV2) -> Result<IngestOutcome, Self::Error> {
        let tx = self.connection.transaction()?;
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
                serde_json::to_string(event)?,
            ],
        )?;

        if inserted == 0 {
            tx.commit()?;
            return Ok(IngestOutcome::Duplicate);
        }

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
        Ok(IngestOutcome::Inserted)
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
            "wechat.ui-parser",
            "scope.personal",
            observed_at,
            json!({"text": "sample ships Monday", "unknown": {"v": 99}}),
            "wechat-parser-v0",
            "opaque storage fixture",
        );
        event.payload_version = 99;
        event.occurred_at = Some(occurred_at);
        event.source_sequence = Some(11);
        event.ingested_at = OffsetDateTime::UNIX_EPOCH;

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
        assert_ne!(stored["ingested_at"], "1970-01-01T00:00:00Z");
    }

    #[test]
    fn latest_source_event_is_a_durable_collector_cursor() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        for sequence in [40, 42, 41] {
            let mut event = EventEnvelopeV2::observed(
                "screenpipe.ui_frame_observed",
                "screenpipe.local",
                "scope.personal",
                OffsetDateTime::from_unix_timestamp(1_700_000_000 + sequence as i64).unwrap(),
                json!({"screenpipe_frame_id": sequence}),
                "screenpipe-rest-v1",
                "fixture",
            );
            event.source_sequence = Some(sequence);
            event.occurred_at = Some(event.observed_at);
            engine.ingest_v2(&event).unwrap();
        }

        let stored = engine
            .repository()
            .latest_raw_event_envelope_for_source("screenpipe.local", "scope.personal")
            .unwrap()
            .unwrap();
        let cursor: EventEnvelopeV2 = serde_json::from_str(&stored).unwrap();
        assert_eq!(cursor.source_sequence, Some(42));
        assert_eq!(cursor.payload["screenpipe_frame_id"], 42);
    }

    #[test]
    fn duplicate_v2_event_cannot_rewrite_the_original_raw_envelope() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = EventEnvelopeV2::observed(
            "future.event",
            "test.collector",
            "scope.personal",
            OffsetDateTime::now_utc(),
            json!({"value": "original"}),
            "test-v1",
            "fixture",
        );
        let mut conflicting_replay = event.clone();
        conflicting_replay.payload = json!({"value": "conflicting replay"});

        assert_eq!(
            engine.ingest_v2(&event).unwrap().outcome,
            IngestOutcome::Inserted
        );
        assert_eq!(
            engine.ingest_v2(&conflicting_replay).unwrap().outcome,
            IngestOutcome::Duplicate
        );

        let stored: serde_json::Value = serde_json::from_str(
            &engine
                .repository()
                .raw_event_envelope_json(event.event_id)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stored["payload"]["value"], "original");
    }
}
