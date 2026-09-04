from pathlib import Path

path = Path("crates/storage-sqlite/src/lib.rs")
text = path.read_text(encoding="utf-8")

old = "const CURRENT_DATABASE_VERSION: u32 = 2;"
if text.count(old) != 1:
    raise SystemExit("database version anchor mismatch")
text = text.replace(old, "const CURRENT_DATABASE_VERSION: u32 = 3;", 1)

old = '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
new = '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", 2)?;\n        tx.commit()?;\n        version = 2;\n    }\n    if version == 2 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V3)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n'''
if text.count(old) != 1:
    raise SystemExit("migration chain anchor mismatch")
text = text.replace(old, new, 1)

schema_v2_end = '''CREATE TABLE collector_checkpoint (\n  source TEXT NOT NULL,\n  scope_id TEXT NOT NULL,\n  last_sequence INTEGER NOT NULL,\n  reconciliation_required INTEGER NOT NULL CHECK(reconciliation_required IN (0, 1)),\n  updated_at TEXT NOT NULL,\n  PRIMARY KEY(source, scope_id)\n);\n"#;\n'''
if text.count(schema_v2_end) != 1:
    raise SystemExit("SCHEMA_V2 tail anchor mismatch")
text = text.replace(
    schema_v2_end,
    schema_v2_end
    + '''\nconst SCHEMA_V3: &str = r#"\nCREATE INDEX IF NOT EXISTS raw_event_scope_observed_cursor\nON raw_event(scope_id, observed_at DESC, event_id DESC);\n"#;\n''',
    1,
)

if "\nmod retrieval;\n" not in text:
    if text.count("\nmod v2;\n") != 1:
        raise SystemExit("v2 module anchor mismatch")
    text = text.replace("\nmod v2;\n", "\nmod retrieval;\nmod v2;\n", 1)

path.write_text(text, encoding="utf-8")
