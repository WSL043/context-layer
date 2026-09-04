from pathlib import Path

# 1. Core retrieval test import.
core = Path("crates/core/src/retrieval.rs")
text = core.read_text(encoding="utf-8")
anchor = "    use serde_json::json;\n\n    use super::*;"
if anchor in text:
    text = text.replace(
        anchor,
        "    use serde_json::json;\n    use time::Duration;\n\n    use super::*;",
        1,
    )
core.write_text(text, encoding="utf-8")

# 2. SQLite retrieval: keep SQL LIMIT in SQLite's signed integer domain.
storage_retrieval = Path("crates/storage-sqlite/src/retrieval.rs")
text = storage_retrieval.read_text(encoding="utf-8")
text = text.replace(
    "        let sql_limit = query.limit.saturating_add(1);",
    "        let fetch_limit = query.limit.saturating_add(1);\n        let sql_limit = i64::try_from(fetch_limit).unwrap_or(i64::MAX);",
    1,
)
text = text.replace(
    "        let mut records = Vec::with_capacity(sql_limit);",
    "        let mut records = Vec::with_capacity(fetch_limit);",
    1,
)
storage_retrieval.write_text(text, encoding="utf-8")

# 3. Idempotently ensure the versioned v3 retrieval index migration and module.
storage = Path("crates/storage-sqlite/src/lib.rs")
text = storage.read_text(encoding="utf-8")
if "const CURRENT_DATABASE_VERSION: u32 = 2;" in text:
    text = text.replace(
        "const CURRENT_DATABASE_VERSION: u32 = 2;",
        "const CURRENT_DATABASE_VERSION: u32 = 3;",
        1,
    )

old_chain = '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
new_chain = '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", 2)?;\n        tx.commit()?;\n        version = 2;\n    }\n    if version == 2 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V3)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
if old_chain in text:
    text = text.replace(old_chain, new_chain, 1)

if "const SCHEMA_V3: &str" not in text:
    tail = '''CREATE TABLE collector_checkpoint (\n  source TEXT NOT NULL,\n  scope_id TEXT NOT NULL,\n  last_sequence INTEGER NOT NULL,\n  reconciliation_required INTEGER NOT NULL CHECK(reconciliation_required IN (0, 1)),\n  updated_at TEXT NOT NULL,\n  PRIMARY KEY(source, scope_id)\n);\n"#;\n'''
    if tail not in text:
        raise SystemExit("SCHEMA_V2 tail anchor missing")
    text = text.replace(
        tail,
        tail
        + '''\nconst SCHEMA_V3: &str = r#"\nCREATE INDEX IF NOT EXISTS raw_event_scope_observed_cursor\nON raw_event(scope_id, observed_at DESC, event_id DESC);\n"#;\n''',
        1,
    )

if "\nmod retrieval;\n" not in text:
    if "\nmod v2;\n" not in text:
        raise SystemExit("v2 module anchor missing")
    text = text.replace("\nmod v2;\n", "\nmod retrieval;\nmod v2;\n", 1)
storage.write_text(text, encoding="utf-8")

# 4. Remove the temporary normalizer job appended to normal CI, if still present.
ci = Path(".github/workflows/ci.yml")
text = ci.read_text(encoding="utf-8")
marker = "\n  normalize-retrieval-migration:\n"
if marker in text:
    text = text.split(marker, 1)[0].rstrip() + "\n"
ci.write_text(text, encoding="utf-8")
