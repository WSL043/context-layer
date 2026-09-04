use context_core::content_access::{RawEventLookup, RawEventLookupRepository};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::*;

impl RawEventLookupRepository for SqliteRepository {
    type Error = StorageError;

    fn raw_event_by_id(&self, event_id: Uuid) -> Result<Option<RawEventLookup>, Self::Error> {
        Ok(self
            .connection
            .query_row(
                "SELECT schema_version, envelope_json
                 FROM raw_event
                 WHERE event_id = ?1",
                [event_id.to_string()],
                |row| {
                    Ok(RawEventLookup {
                        event_id,
                        schema_version: row.get(0)?,
                        envelope_json: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use context_contracts::EventEnvelopeV2;
    use context_core::{ContextEngine, content_access::RawEventLookupRepository};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn raw_event_lookup_is_by_exact_event_id() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = EventEnvelopeV2::observed(
            "fixture.lookup",
            "fixture",
            "scope.personal",
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            json!({"fixture": true}),
            "fixture",
            "raw lookup fixture",
        );
        engine.ingest_v2(&event).unwrap();

        let found = engine
            .repository()
            .raw_event_by_id(event.event_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.event_id, event.event_id);
        assert_eq!(found.schema_version, 2);
        assert!(found.envelope_json.contains("fixture.lookup"));
        assert!(engine
            .repository()
            .raw_event_by_id(Uuid::now_v7())
            .unwrap()
            .is_none());
    }
}
