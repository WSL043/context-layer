use context_contracts::{EventEnvelopeV2, ScopeId};
use context_core::{
    ContextEngine,
    retrieval::{RawTimelineQuery, TimelineCursor, TimelineRepository},
};
use context_storage_sqlite::SqliteRepository;
use rusqlite::Connection;
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn raw_event_at(observed_at: OffsetDateTime) -> EventEnvelopeV2 {
    EventEnvelopeV2::observed(
        "fixture.timeline",
        "fixture.timeline",
        "scope.personal",
        observed_at,
        json!({"instant": observed_at.unix_timestamp_nanos().to_string()}),
        "fixture",
        "sortable-time regression fixture",
    )
}

#[test]
fn fractional_second_orders_after_whole_second_and_keyset_pages_stably() {
    let repository = SqliteRepository::in_memory().unwrap();
    let mut engine = ContextEngine::new(repository);
    let whole = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let fractional = whole + Duration::milliseconds(500);
    let whole_event = raw_event_at(whole);
    let fractional_event = raw_event_at(fractional);
    engine.ingest_v2(&whole_event).unwrap();
    engine.ingest_v2(&fractional_event).unwrap();

    let first = engine
        .repository()
        .query_raw_timeline(&RawTimelineQuery {
            scope_id: ScopeId("scope.personal".into()),
            start_at: None,
            end_at: None,
            before: None,
            limit: 1,
        })
        .unwrap();
    assert_eq!(first.records.len(), 1);
    assert!(first.has_more);
    assert_eq!(first.records[0].event_id, fractional_event.event_id);
    assert_eq!(first.records[0].observed_at, fractional);

    let second = engine
        .repository()
        .query_raw_timeline(&RawTimelineQuery {
            scope_id: ScopeId("scope.personal".into()),
            start_at: None,
            end_at: None,
            before: Some(TimelineCursor {
                observed_at: first.records[0].observed_at,
                event_id: first.records[0].event_id,
            }),
            limit: 1,
        })
        .unwrap();
    assert_eq!(second.records.len(), 1);
    assert!(!second.has_more);
    assert_eq!(second.records[0].event_id, whole_event.event_id);
    assert_eq!(second.records[0].observed_at, whole);
}

#[test]
fn version_three_database_backfills_fixed_utc_keys_and_replaces_text_time_index() {
    let path = std::env::temp_dir().join(format!("context-sortable-time-v4-{}.db", Uuid::now_v7()));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE raw_event (
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
            CREATE INDEX raw_event_scope_observed_cursor
            ON raw_event(scope_id, observed_at DESC, event_id DESC);
            INSERT INTO raw_event (
              event_id, schema_version, source, observed_at, ingested_at,
              scope_id, sensitivity, envelope_json
            ) VALUES
              ('018c4a15-6f80-7000-8000-000000000001', 2, 'fixture',
               '2023-11-14T22:13:20Z', '2023-11-14T22:13:21Z',
               'scope.personal', 'metadata', '{}'),
              ('018c4a15-6f80-7000-8000-000000000002', 2, 'fixture',
               '2023-11-14T22:13:20.5Z', '2023-11-14T22:13:21Z',
               'scope.personal', 'metadata', '{}');
            PRAGMA user_version = 3;
            "#,
        )
        .unwrap();
    drop(connection);

    let repository = SqliteRepository::open(&path).unwrap();
    drop(repository);

    let connection = Connection::open(&path).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);

    let mut statement = connection
        .prepare("SELECT observed_key FROM raw_event ORDER BY observed_key ASC")
        .unwrap();
    let keys = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        keys,
        vec![
            "2023-11-14T22:13:20.000000000Z",
            "2023-11-14T22:13:20.500000000Z",
        ]
    );
    drop(statement);

    let old_index_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'raw_event_scope_observed_cursor'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_index_count, 0);
    let new_index_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'raw_event_scope_observed_key_cursor'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_index_count, 1);

    drop(connection);
    std::fs::remove_file(path).unwrap();
}
