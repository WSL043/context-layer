from pathlib import Path

lib = Path("crates/storage-sqlite/src/lib.rs")
text = lib.read_text(encoding="utf-8")

text = text.replace(
    "const CURRENT_DATABASE_VERSION: u32 = 3;",
    "const CURRENT_DATABASE_VERSION: u32 = 4;",
    1,
)

error_anchor = '''    #[error("database schema version {actual} is newer than supported version {supported}")]\n    UnsupportedDatabaseVersion { actual: u32, supported: u32 },\n'''
if "TimestampOutOfSortableRange" not in text:
    if text.count(error_anchor) != 1:
        raise SystemExit("storage error anchor mismatch")
    text = text.replace(
        error_anchor,
        error_anchor
        + '''    #[error("timestamp year {year} cannot be represented in sortable UTC key")]\n    TimestampOutOfSortableRange { year: i32 },\n''',
        1,
    )

format_anchor = '''fn format_time(value: OffsetDateTime) -> Result<String, time::error::Format> {\n    value.format(&Rfc3339)\n}\n'''
if "fn format_sort_key(" not in text:
    if text.count(format_anchor) != 1:
        raise SystemExit("format_time anchor mismatch")
    text = text.replace(
        format_anchor,
        format_anchor
        + '''\nfn format_sort_key(value: OffsetDateTime) -> Result<String, StorageError> {\n    let utc = value.to_offset(time::UtcOffset::UTC);\n    let year = utc.year();\n    if !(0..=9999).contains(&year) {\n        return Err(StorageError::TimestampOutOfSortableRange { year });\n    }\n    Ok(format!(\n        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",\n        utc.month() as u8,\n        utc.day(),\n        utc.hour(),\n        utc.minute(),\n        utc.second(),\n        utc.nanosecond(),\n    ))\n}\n''',
        1,
    )

append_signature = '''    fn append_with_projection(\n        &mut self,\n        event: &EventEnvelope,\n        commands: &[ProjectionCommand],\n    ) -> Result<IngestOutcome, Self::Error> {\n        let tx = self.connection.transaction()?;\n'''
append_region = text.split("impl EventRepository for SqliteRepository", 1)[1].split(
    "fn find_download_match", 1
)[0]
if "let observed_key = format_sort_key(event.observed_at)?;" not in append_region:
    if text.count(append_signature) != 1:
        raise SystemExit("v1 append anchor mismatch")
    text = text.replace(
        append_signature,
        append_signature.replace(
            "        let tx",
            "        let observed_key = format_sort_key(event.observed_at)?;\n        let tx",
        ),
        1,
    )

v1_sql_old = '''                event_id, schema_version, source, source_sequence, observed_at,\n                ingested_at, scope_id, correlation_id, sensitivity, envelope_json\n             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)'''
v1_sql_new = '''                event_id, schema_version, source, source_sequence, observed_at, observed_key,\n                ingested_at, scope_id, correlation_id, sensitivity, envelope_json\n             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)'''
if v1_sql_old in text:
    text = text.replace(v1_sql_old, v1_sql_new, 1)

v1_params_old = '''                format_time(event.observed_at)?,\n                format_time(event.ingested_at)?,'''
v1_params_new = '''                format_time(event.observed_at)?,\n                observed_key,\n                format_time(event.ingested_at)?,'''
if v1_params_old in text:
    text = text.replace(v1_params_old, v1_params_new, 1)

old_v2_block = '''    if version == 2 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V3)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
new_v2_block = '''    if version == 2 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V3)?;\n        tx.pragma_update(None, "user_version", 3)?;\n        tx.commit()?;\n        version = 3;\n    }\n    if version == 3 {\n        let tx = connection.transaction()?;\n        tx.execute_batch("ALTER TABLE raw_event ADD COLUMN observed_key TEXT;")?;\n        backfill_observed_keys(&tx)?;\n        tx.execute_batch(SCHEMA_V4)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
if old_v2_block in text:
    text = text.replace(old_v2_block, new_v2_block, 1)
elif "if version == 3 {" not in text:
    raise SystemExit("migration v2 block mismatch")

if "fn backfill_observed_keys(" not in text:
    migrate_end = '''    Ok(())\n}\n\nconst SCHEMA_V1: &str = r#"'''
    if text.count(migrate_end) != 1:
        raise SystemExit("migrate end anchor mismatch")
    helper = '''    Ok(())\n}\n\nfn backfill_observed_keys(tx: &Transaction<'_>) -> Result<(), StorageError> {\n    let rows = {\n        let mut statement = tx.prepare(\n            "SELECT event_id, observed_at FROM raw_event WHERE observed_key IS NULL",\n        )?;\n        statement\n            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?\n            .collect::<Result<Vec<_>, _>>()?\n    };\n    for (event_id, observed_at) in rows {\n        let observed_at = OffsetDateTime::parse(&observed_at, &Rfc3339)?;\n        let observed_key = format_sort_key(observed_at)?;\n        tx.execute(\n            "UPDATE raw_event SET observed_key = ?2 WHERE event_id = ?1",\n            params![event_id, observed_key],\n        )?;\n    }\n    Ok(())\n}\n\nconst SCHEMA_V1: &str = r#"'''
    text = text.replace(migrate_end, helper, 1)

schema_v3 = '''const SCHEMA_V3: &str = r#"\nCREATE INDEX IF NOT EXISTS raw_event_scope_observed_cursor\nON raw_event(scope_id, observed_at DESC, event_id DESC);\n"#;\n'''
if "const SCHEMA_V4:" not in text:
    if text.count(schema_v3) != 1:
        raise SystemExit("schema v3 anchor mismatch")
    text = text.replace(
        schema_v3,
        schema_v3
        + '''\nconst SCHEMA_V4: &str = r#"\nDROP INDEX IF EXISTS raw_event_scope_observed_cursor;\nCREATE INDEX IF NOT EXISTS raw_event_scope_observed_key_cursor\nON raw_event(scope_id, observed_key DESC, event_id DESC);\nCREATE TRIGGER IF NOT EXISTS raw_event_observed_key_required\nBEFORE INSERT ON raw_event\nWHEN NEW.observed_key IS NULL\nBEGIN\n  SELECT RAISE(ABORT, 'raw_event.observed_key is required');\nEND;\n"#;\n''',
        1,
    )

lib.write_text(text, encoding="utf-8")

v2 = Path("crates/storage-sqlite/src/v2.rs")
text = v2.read_text(encoding="utf-8")
anchor = '''    fn append_raw_v2(&mut self, event: &EventEnvelopeV2) -> Result<IngestOutcome, Self::Error> {\n        let tx = self.connection.transaction()?;\n'''
if "let observed_key = format_sort_key(event.observed_at)?;" not in text:
    if text.count(anchor) != 1:
        raise SystemExit("v2 append anchor mismatch")
    text = text.replace(
        anchor,
        anchor.replace(
            "        let tx",
            "        let observed_key = format_sort_key(event.observed_at)?;\n        let tx",
        ),
        1,
    )
old = '''                event_id, schema_version, source, source_sequence, observed_at,\n                ingested_at, scope_id, correlation_id, sensitivity, envelope_json\n             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)'''
new = '''                event_id, schema_version, source, source_sequence, observed_at, observed_key,\n                ingested_at, scope_id, correlation_id, sensitivity, envelope_json\n             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)'''
if old in text:
    text = text.replace(old, new, 1)
old = '''                format_time(event.observed_at)?,\n                format_time(event.ingested_at)?,'''
new = '''                format_time(event.observed_at)?,\n                observed_key,\n                format_time(event.ingested_at)?,'''
if old in text:
    text = text.replace(old, new, 1)
v2.write_text(text, encoding="utf-8")

retrieval = Path("crates/storage-sqlite/src/retrieval.rs")
text = retrieval.read_text(encoding="utf-8")
text = text.replace(
    "query.start_at.map(format_time).transpose()?",
    "query.start_at.map(format_sort_key).transpose()?",
)
text = text.replace(
    "query.end_at.map(format_time).transpose()?",
    "query.end_at.map(format_sort_key).transpose()?",
)
text = text.replace(
    "map(|cursor| format_time(cursor.observed_at))",
    "map(|cursor| format_sort_key(cursor.observed_at))",
)
text = text.replace("observed_at >= ?2", "observed_key >= ?2")
text = text.replace("observed_at < ?3", "observed_key < ?3")
text = text.replace("observed_at < ?4", "observed_key < ?4")
text = text.replace("observed_at = ?4", "observed_key = ?4")
text = text.replace(
    "ORDER BY observed_at DESC, event_id DESC",
    "ORDER BY observed_key DESC, event_id DESC",
)
retrieval.write_text(text, encoding="utf-8")

legacy_test = Path("crates/storage-sqlite/tests/retrieval_v3.rs")
if legacy_test.exists():
    text = legacy_test.read_text(encoding="utf-8")
    text = text.replace(
        "version_two_database_migrates_to_v3_retrieval_index",
        "version_two_database_migrates_through_sortable_retrieval_index",
    )
    text = text.replace("assert_eq!(version, 3);", "assert_eq!(version, 4);")
    text = text.replace(
        "name = 'raw_event_scope_observed_cursor'",
        "name = 'raw_event_scope_observed_key_cursor'",
    )
    legacy_test.write_text(text, encoding="utf-8")

ci = Path(".github/workflows/ci.yml")
ci_text = ci.read_text(encoding="utf-8")
marker = "\n  normalize-sortable-time-key:\n"
if marker in ci_text:
    ci_text = ci_text.split(marker, 1)[0].rstrip() + "\n"
ci.write_text(ci_text, encoding="utf-8")
