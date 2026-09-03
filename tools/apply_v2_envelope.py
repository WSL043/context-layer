from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


storage_path = Path("crates/storage-sqlite/src/lib.rs")
storage = storage_path.read_text(encoding="utf-8")

storage = replace_once(
    storage,
    "use context_contracts::{EventEnvelope, FileIdentity};",
    "use context_contracts::{EventEnvelope, EventEnvelopeV2, FileIdentity};",
    "storage imports",
)

storage = replace_once(
    storage,
    '''    pub fn raw_event_count(&self) -> Result<u64, StorageError> {\n        Ok(self\n            .connection\n            .query_row("SELECT COUNT(*) FROM raw_event", [], |row| row.get(0))?)\n    }\n''',
    '''    pub fn raw_event_count(&self) -> Result<u64, StorageError> {\n        Ok(self\n            .connection\n            .query_row("SELECT COUNT(*) FROM raw_event", [], |row| row.get(0))?)\n    }\n\n    pub fn raw_v2_event_count(&self) -> Result<u64, StorageError> {\n        Ok(self.connection.query_row(\n            "SELECT COUNT(*) FROM raw_event WHERE envelope_version = 2",\n            [],\n            |row| row.get(0),\n        )?)\n    }\n\n    pub fn raw_event_envelope_json(&self, event_id: Uuid) -> Result<Option<String>, StorageError> {\n        Ok(self\n            .connection\n            .query_row(\n                "SELECT envelope_json FROM raw_event WHERE event_id = ?1",\n                [event_id.to_string()],\n                |row| row.get(0),\n            )\n            .optional()?)\n    }\n''',
    "storage query helpers",
)

anchor = '''    fn find_download_match(\n        &self,\n        path: &str,\n        tolerance_seconds: i64,\n    ) -> Result<Option<DownloadMatchCandidate>, Self::Error> {\n'''
append_v2 = '''    fn append_raw_v2(&mut self, event: &EventEnvelopeV2) -> Result<IngestOutcome, Self::Error> {\n        let tx = self.connection.transaction()?;\n        let inserted = tx.execute(\n            "INSERT OR IGNORE INTO raw_event (\n                event_id, schema_version, envelope_version, event_type, payload_version,\n                source, source_sequence, occurred_at, observed_at, ingested_at, device_id,\n                scope_id, correlation_id, sensitivity, content_refs_json, envelope_json\n             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",\n            params![\n                event.event_id.to_string(),\n                event.envelope_version,\n                event.envelope_version,\n                event.event_type,\n                event.payload_version,\n                event.source.0,\n                event.source_sequence,\n                event.occurred_at.map(format_time).transpose()?,\n                format_time(event.observed_at)?,\n                format_time(event.ingested_at)?,\n                event.device_id,\n                event.scope_id.0,\n                event.correlation_id.map(|value| value.to_string()),\n                serde_json::to_string(&event.sensitivity)?,\n                serde_json::to_string(&event.content_refs)?,\n                serde_json::to_string(event)?,\n            ],\n        )?;\n\n        if let Some(sequence) = event.source_sequence {\n            let requires_reconciliation = event.event_type == "collector.gap";\n            tx.execute(\n                "INSERT INTO collector_checkpoint (\n                    source, scope_id, last_sequence, reconciliation_required, updated_at\n                 ) VALUES (?1, ?2, ?3, ?4, ?5)\n                 ON CONFLICT(source, scope_id) DO UPDATE SET\n                   last_sequence = MAX(last_sequence, excluded.last_sequence),\n                   reconciliation_required = MAX(\n                     reconciliation_required, excluded.reconciliation_required\n                   ),\n                   updated_at = excluded.updated_at",\n                params![\n                    event.source.0,\n                    event.scope_id.0,\n                    sequence,\n                    requires_reconciliation,\n                    format_time(event.ingested_at)?,\n                ],\n            )?;\n        }\n\n        tx.commit()?;\n        Ok(if inserted == 0 {\n            IngestOutcome::Duplicate\n        } else {\n            IngestOutcome::Inserted\n        })\n    }\n\n'''
storage = replace_once(storage, anchor, append_v2 + anchor, "append raw v2")

storage = replace_once(
    storage,
    "const CURRENT_DATABASE_VERSION: u32 = 2;",
    "const CURRENT_DATABASE_VERSION: u32 = 3;",
    "database version",
)

storage = replace_once(
    storage,
    '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n''',
    '''    if version == 1 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V2)?;\n        tx.pragma_update(None, "user_version", 2)?;\n        tx.commit()?;\n        version = 2;\n    }\n    if version == 2 {\n        let tx = connection.transaction()?;\n        tx.execute_batch(SCHEMA_V3)?;\n        tx.pragma_update(None, "user_version", CURRENT_DATABASE_VERSION)?;\n        tx.commit()?;\n    }\n    Ok(())\n}\n''',
    "migration chain",
)

storage = replace_once(
    storage,
    '''const SCHEMA_V2: &str = r#"\nALTER TABLE location ADD COLUMN scope_id TEXT NOT NULL DEFAULT '';\n\nUPDATE location\nSET scope_id = COALESCE(\n  (SELECT raw_event.scope_id\n   FROM raw_event\n   WHERE raw_event.event_id = location.source_event_id),\n  ''\n);\n\nCREATE TABLE collector_checkpoint (\n  source TEXT NOT NULL,\n  scope_id TEXT NOT NULL,\n  last_sequence INTEGER NOT NULL,\n  reconciliation_required INTEGER NOT NULL CHECK(reconciliation_required IN (0, 1)),\n  updated_at TEXT NOT NULL,\n  PRIMARY KEY(source, scope_id)\n);\n"#;\n''',
    '''const SCHEMA_V2: &str = r#"\nALTER TABLE location ADD COLUMN scope_id TEXT NOT NULL DEFAULT '';\n\nUPDATE location\nSET scope_id = COALESCE(\n  (SELECT raw_event.scope_id\n   FROM raw_event\n   WHERE raw_event.event_id = location.source_event_id),\n  ''\n);\n\nCREATE TABLE collector_checkpoint (\n  source TEXT NOT NULL,\n  scope_id TEXT NOT NULL,\n  last_sequence INTEGER NOT NULL,\n  reconciliation_required INTEGER NOT NULL CHECK(reconciliation_required IN (0, 1)),\n  updated_at TEXT NOT NULL,\n  PRIMARY KEY(source, scope_id)\n);\n"#;\n\nconst SCHEMA_V3: &str = r#"\nALTER TABLE raw_event ADD COLUMN envelope_version INTEGER NOT NULL DEFAULT 1;\nALTER TABLE raw_event ADD COLUMN event_type TEXT;\nALTER TABLE raw_event ADD COLUMN payload_version INTEGER;\nALTER TABLE raw_event ADD COLUMN occurred_at TEXT;\nALTER TABLE raw_event ADD COLUMN device_id TEXT;\nALTER TABLE raw_event ADD COLUMN content_refs_json TEXT;\n\nCREATE INDEX raw_event_v2_type_time\nON raw_event(event_type, observed_at)\nWHERE envelope_version = 2;\n"#;\n''',
    "schema v3",
)

storage = replace_once(
    storage,
    "    use context_contracts::{EventEnvelope, EventPayload, FileChange, FileIdentity};",
    "    use context_contracts::{EventEnvelope, EventEnvelopeV2, EventPayload, FileChange, FileIdentity};\n    use serde_json::json;",
    "storage test imports",
)

storage_test_anchor = '''    #[test]\n    fn duplicate_event_is_idempotent() {\n'''
storage_test = '''    #[test]\n    fn opaque_v2_event_is_retained_without_projection() {\n        let repository = SqliteRepository::in_memory().unwrap();\n        let mut engine = ContextEngine::new(repository);\n        let occurred_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();\n        let observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_120).unwrap();\n        let mut event = EventEnvelopeV2::observed(\n            "wechat.message",\n            99,\n            "wechat.ui-parser",\n            "scope.personal",\n            observed_at,\n            json!({"text": "sample ships Monday", "unknown": {"v": 99}}),\n            "wechat-parser-v0",\n            "opaque storage fixture",\n        );\n        event.occurred_at = Some(occurred_at);\n        event.source_sequence = Some(11);\n\n        assert_eq!(\n            engine.ingest_v2(&event).unwrap().outcome,\n            IngestOutcome::Inserted\n        );\n        assert_eq!(\n            engine.ingest_v2(&event).unwrap().outcome,\n            IngestOutcome::Duplicate\n        );\n        assert_eq!(engine.repository().raw_v2_event_count().unwrap(), 1);\n        assert_eq!(\n            engine\n                .repository()\n                .last_source_sequence("wechat.ui-parser", "scope.personal")\n                .unwrap(),\n            Some(11)\n        );\n\n        let json: serde_json::Value = serde_json::from_str(\n            &engine\n                .repository()\n                .raw_event_envelope_json(event.event_id)\n                .unwrap()\n                .unwrap(),\n        )\n        .unwrap();\n        assert_eq!(json["event_type"], "wechat.message");\n        assert_eq!(json["payload_version"], 99);\n        assert_eq!(json["payload"]["unknown"]["v"], 99);\n        assert_eq!(json["occurred_at"], "2023-11-14T22:13:20Z");\n        assert_eq!(json["observed_at"], "2023-11-14T22:15:20Z");\n    }\n\n'''
storage = replace_once(storage, storage_test_anchor, storage_test + storage_test_anchor, "v2 storage test")

storage_path.write_text(storage, encoding="utf-8")

main_path = Path("apps/context-agent/src/main.rs")
main = main_path.read_text(encoding="utf-8")
main = replace_once(
    main,
    "use context_contracts::{EventEnvelope, EventPayload, FileChange};",
    "use context_contracts::{EventEnvelope, EventEnvelopeV2, EventPayload, FileChange};",
    "agent imports",
)

main = replace_once(
    main,
    '''            LocalApiCommand::SubmitEvent { event } => match engine.ingest(&event) {\n                Ok(report) => LocalApiResult::EventAccepted {\n                    event_id: event.event_id,\n                    duplicate: report.outcome == IngestOutcome::Duplicate,\n                },\n                Err(error) => LocalApiResult::Error {\n                    code: "ingest_failed".into(),\n                    message: error.to_string(),\n                },\n            },\n''',
    '''            LocalApiCommand::SubmitEvent { event } => match engine.ingest(&event) {\n                Ok(report) => LocalApiResult::EventAccepted {\n                    event_id: event.event_id,\n                    duplicate: report.outcome == IngestOutcome::Duplicate,\n                },\n                Err(error) => LocalApiResult::Error {\n                    code: "ingest_failed".into(),\n                    message: error.to_string(),\n                },\n            },\n            LocalApiCommand::SubmitEventV2 { event } => match engine.ingest_v2(&event) {\n                Ok(report) => LocalApiResult::EventAccepted {\n                    event_id: event.event_id,\n                    duplicate: report.outcome == IngestOutcome::Duplicate,\n                },\n                Err(error) => LocalApiResult::Error {\n                    code: "ingest_failed".into(),\n                    message: error.to_string(),\n                },\n            },\n''',
    "agent v2 route",
)

main_test_anchor = '''    #[test]\n    fn submit_event_request_is_ingested_and_acknowledged() {\n'''
main_test = '''    #[test]\n    fn submit_v2_event_request_is_retained_and_acknowledged() {\n        let repository = SqliteRepository::in_memory().unwrap();\n        let mut engine = ContextEngine::new(repository);\n        let event = EventEnvelopeV2::observed(\n            "ui.window_focused",\n            7,\n            "windows.foreground",\n            "scope.personal",\n            OffsetDateTime::now_utc(),\n            serde_json::json!({"process": "notepad.exe", "future": true}),\n            "foreground-v0",\n            "fixture",\n        );\n        let event_id = event.event_id;\n        let request_id = Uuid::now_v7();\n        let response = handle_request(\n            &mut engine,\n            LocalApiRequest {\n                request_id,\n                protocol_version: LOCAL_API_VERSION,\n                command: LocalApiCommand::SubmitEventV2 {\n                    event: Box::new(event),\n                },\n            },\n        );\n\n        assert_eq!(response.request_id, request_id);\n        assert!(matches!(\n            response.result,\n            LocalApiResult::EventAccepted {\n                event_id: accepted,\n                duplicate: false\n            } if accepted == event_id\n        ));\n        assert_eq!(engine.repository().raw_v2_event_count().unwrap(), 1);\n    }\n\n'''
main = replace_once(main, main_test_anchor, main_test + main_test_anchor, "agent v2 test")
main_path.write_text(main, encoding="utf-8")
