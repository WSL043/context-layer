use context_contracts::{ScopeId, SensitivityClass};
use context_core::content_access::{RawEventLookup, RawEventLookupRepository};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::*;

impl RawEventLookupRepository for SqliteRepository {
    type Error = StorageError;

    fn raw_event_by_id(&self, event_id: Uuid) -> Result<Option<RawEventLookup>, Self::Error> {
        let row = self
            .connection
            .query_row(
                "SELECT schema_version, scope_id, sensitivity, envelope_json
                 FROM raw_event
                 WHERE event_id = ?1",
                [event_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, u16>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(schema_version, scope_id, sensitivity, envelope_json)| {
            let sensitivity = serde_json::from_str::<SensitivityClass>(&sensitivity)?;
            Ok::<RawEventLookup, StorageError>(RawEventLookup {
                event_id,
                schema_version,
                scope_id: ScopeId(scope_id),
                sensitivity,
                envelope_json,
            })
        })
        .transpose()
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
    fn raw_event_lookup_is_by_exact_event_id_with_authorization_metadata() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut event = EventEnvelopeV2::observed(
            "fixture.lookup",
            "fixture",
            "scope.personal",
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            json!({"fixture": true}),
            "fixture",
            "raw lookup fixture",
        );
        event.sensitivity = SensitivityClass::Sensitive;
        engine.ingest_v2(&event).unwrap();

        let found = engine
            .repository()
            .raw_event_by_id(event.event_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.event_id, event.event_id);
        assert_eq!(found.schema_version, 2);
        assert_eq!(found.scope_id, ScopeId("scope.personal".into()));
        assert_eq!(found.sensitivity, SensitivityClass::Sensitive);
        assert!(found.envelope_json.contains("fixture.lookup"));
        assert!(
            engine
                .repository()
                .raw_event_by_id(Uuid::now_v7())
                .unwrap()
                .is_none()
        );
    }
}
