use context_contracts::ScopeId;
use context_core::retrieval::{RawTimelineQuery, TimelineRepository};
use context_storage_sqlite::SqliteRepository;
use rusqlite::Connection;
use uuid::Uuid;

#[test]
fn version_two_database_migrates_to_v3_retrieval_index() {
    let path = std::env::temp_dir().join(format!("context-retrieval-v3-{}.db", Uuid::now_v7()));
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
            PRAGMA user_version = 2;
            "#,
        )
        .unwrap();
    drop(connection);

    let repository = SqliteRepository::open(&path).unwrap();
    let page = repository
        .query_raw_timeline(&RawTimelineQuery {
            scope_id: ScopeId("scope.personal".into()),
            start_at: None,
            end_at: None,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert!(page.records.is_empty());
    drop(repository);

    let connection = Connection::open(&path).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let index_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'raw_event_scope_observed_cursor'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1);

    drop(connection);
    std::fs::remove_file(path).unwrap();
}
