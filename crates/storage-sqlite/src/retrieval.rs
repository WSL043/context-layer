use context_core::retrieval::{
    RawTimelinePage, RawTimelineQuery, RawTimelineRecord, TimelineRepository,
};
use rusqlite::params;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::*;

impl TimelineRepository for SqliteRepository {
    type Error = StorageError;

    fn query_raw_timeline(
        &self,
        query: &RawTimelineQuery,
    ) -> Result<RawTimelinePage, Self::Error> {
        let start_at = query.start_at.map(format_time).transpose()?;
        let end_at = query.end_at.map(format_time).transpose()?;
        let cursor_at = query
            .before
            .as_ref()
            .map(|cursor| format_time(cursor.observed_at))
            .transpose()?;
        let cursor_id = query
            .before
            .as_ref()
            .map(|cursor| cursor.event_id.to_string());
        let sql_limit = query.limit.saturating_add(1);

        let mut statement = self.connection.prepare(
            "SELECT event_id, schema_version, observed_at, envelope_json
             FROM raw_event
             WHERE scope_id = ?1
               AND (?2 IS NULL OR observed_at >= ?2)
               AND (?3 IS NULL OR observed_at < ?3)
               AND (
                 ?4 IS NULL
                 OR observed_at < ?4
                 OR (observed_at = ?4 AND event_id < ?5)
               )
             ORDER BY observed_at DESC, event_id DESC
             LIMIT ?6",
        )?;
        let rows = statement.query_map(
            params![
                query.scope_id.0.as_str(),
                start_at,
                end_at,
                cursor_at,
                cursor_id,
                sql_limit,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u16>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;

        let mut records = Vec::with_capacity(sql_limit);
        for row in rows {
            let (event_id, schema_version, observed_at, envelope_json) = row?;
            records.push(RawTimelineRecord {
                event_id: Uuid::parse_str(&event_id)?,
                schema_version,
                observed_at: OffsetDateTime::parse(&observed_at, &Rfc3339)?,
                envelope_json,
            });
        }
        let has_more = records.len() > query.limit;
        records.truncate(query.limit);
        Ok(RawTimelinePage { records, has_more })
    }
}

#[cfg(test)]
mod tests {
    use context_contracts::{EventEnvelopeV2, ScopeId};
    use context_core::{ContextEngine, retrieval::TimelineCursor};
    use serde_json::json;

    use super::*;

    fn event_at(second: i64) -> EventEnvelopeV2 {
        EventEnvelopeV2::observed(
            "fixture.timeline",
            "fixture.timeline",
            "scope.personal",
            OffsetDateTime::from_unix_timestamp(second).unwrap(),
            json!({"second": second}),
            "fixture",
            "timeline query fixture",
        )
    }

    #[test]
    fn keyset_pages_do_not_repeat_or_skip_equal_timestamps() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let timestamp = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let mut ids = Vec::new();
        for _ in 0..3 {
            let event = event_at(timestamp.unix_timestamp());
            ids.push(event.event_id);
            engine.ingest_v2(&event).unwrap();
        }

        let first = engine
            .repository()
            .query_raw_timeline(&RawTimelineQuery {
                scope_id: ScopeId("scope.personal".into()),
                start_at: None,
                end_at: None,
                before: None,
                limit: 2,
            })
            .unwrap();
        assert_eq!(first.records.len(), 2);
        assert!(first.has_more);
        let cursor_record = first.records.last().unwrap();
        let second = engine
            .repository()
            .query_raw_timeline(&RawTimelineQuery {
                scope_id: ScopeId("scope.personal".into()),
                start_at: None,
                end_at: None,
                before: Some(TimelineCursor {
                    observed_at: cursor_record.observed_at,
                    event_id: cursor_record.event_id,
                }),
                limit: 2,
            })
            .unwrap();
        assert_eq!(second.records.len(), 1);
        assert!(!second.has_more);

        let mut observed = first
            .records
            .iter()
            .chain(second.records.iter())
            .map(|record| record.event_id)
            .collect::<Vec<_>>();
        observed.sort();
        ids.sort();
        assert_eq!(observed, ids);
    }

    #[test]
    fn timeline_query_respects_scope_and_half_open_time_range() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        for second in 100..103 {
            let event = event_at(second);
            engine.ingest_v2(&event).unwrap();
        }
        let mut other = event_at(101);
        other.scope_id = ScopeId("scope.other".into());
        engine.ingest_v2(&other).unwrap();

        let page = engine
            .repository()
            .query_raw_timeline(&RawTimelineQuery {
                scope_id: ScopeId("scope.personal".into()),
                start_at: Some(OffsetDateTime::from_unix_timestamp(101).unwrap()),
                end_at: Some(OffsetDateTime::from_unix_timestamp(103).unwrap()),
                before: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.records.len(), 2);
        assert!(page
            .records
            .iter()
            .all(|record| record.observed_at.unix_timestamp() >= 101));
        assert!(page
            .records
            .iter()
            .all(|record| record.observed_at.unix_timestamp() < 103));
    }
}
