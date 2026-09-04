from pathlib import Path

path = Path("apps/context-agent/src/read_capability.rs")
text = path.read_text(encoding="utf-8")

old = '''fn map_wire_page(\n    entries: Vec<TimelineEntry>,\n    core_next_cursor: Option<TimelineCursor>,\n    grant: RetrievalGrant,\n) -> Result<LocalTimelinePage, ReadRequestError> {\n    let mut wire_entries = Vec::with_capacity(entries.len());\n    let mut forced_cursor = None;\n\n    for entry in entries {\n        let entry_cursor = LocalTimelineCursor {\n            observed_at: entry.observed_at,\n            event_id: entry.event_id,\n        };\n        let wire_entry = map_wire_entry(entry, grant)?;\n        wire_entries.push(wire_entry);\n\n        let candidate = LocalTimelinePage {\n            entries: wire_entries.clone(),\n            next_cursor: None,\n        };\n        let size = serde_json::to_vec(&candidate)\n            .map_err(|error| ReadRequestError::QueryFailed(error.to_string()))?\n            .len();\n        if size > MAX_WIRE_PAGE_BYTES {\n            wire_entries.pop();\n            if wire_entries.is_empty() {\n                return Err(ReadRequestError::QueryFailed(\n                    "a timeline entry exceeds the bounded local API response budget".into(),\n                ));\n            }\n            forced_cursor = wire_entries.last().map(|entry| LocalTimelineCursor {\n                observed_at: entry.observed_at,\n                event_id: entry.event_id,\n            });\n            break;\n        }\n        forced_cursor = Some(entry_cursor);\n    }\n\n    let next_cursor = if wire_entries.len() < entries_len_hint(&wire_entries, &forced_cursor) {\n        forced_cursor\n    } else {\n        core_next_cursor.map(|cursor| LocalTimelineCursor {\n            observed_at: cursor.observed_at,\n            event_id: cursor.event_id,\n        })\n    };\n\n    Ok(LocalTimelinePage {\n        entries: wire_entries,\n        next_cursor,\n    })\n}\n\n// Keeps the page mapping branch explicit without exposing an unbounded page-size calculation.\nfn entries_len_hint(entries: &[LocalTimelineEntry], cursor: &Option<LocalTimelineCursor>) -> usize {\n    entries.len() + usize::from(cursor.is_some())\n}\n'''
new = '''fn map_wire_page(\n    entries: Vec<TimelineEntry>,\n    core_next_cursor: Option<TimelineCursor>,\n    grant: RetrievalGrant,\n) -> Result<LocalTimelinePage, ReadRequestError> {\n    let mut wire_entries = Vec::with_capacity(entries.len());\n    let mut wire_truncated = false;\n\n    for entry in entries {\n        let wire_entry = map_wire_entry(entry, grant)?;\n        wire_entries.push(wire_entry);\n\n        let candidate = LocalTimelinePage {\n            entries: wire_entries.clone(),\n            next_cursor: None,\n        };\n        let size = serde_json::to_vec(&candidate)\n            .map_err(|error| ReadRequestError::QueryFailed(error.to_string()))?\n            .len();\n        if size > MAX_WIRE_PAGE_BYTES {\n            wire_entries.pop();\n            if wire_entries.is_empty() {\n                return Err(ReadRequestError::QueryFailed(\n                    "a timeline entry exceeds the bounded local API response budget".into(),\n                ));\n            }\n            wire_truncated = true;\n            break;\n        }\n    }\n\n    let next_cursor = if wire_truncated {\n        wire_entries.last().map(|entry| LocalTimelineCursor {\n            observed_at: entry.observed_at,\n            event_id: entry.event_id,\n        })\n    } else {\n        core_next_cursor.map(|cursor| LocalTimelineCursor {\n            observed_at: cursor.observed_at,\n            event_id: cursor.event_id,\n        })\n    };\n\n    Ok(LocalTimelinePage {\n        entries: wire_entries,\n        next_cursor,\n    })\n}\n'''
if text.count(old) != 1:
    raise SystemExit("wire page anchor mismatch")
text = text.replace(old, new, 1)

needle = '''        assert_eq!(\n            page.entries[0].payload.as_ref().unwrap()["url"],\n            "https://private.test"\n        );\n'''
replacement = needle + '''        assert!(page.next_cursor.is_none());\n'''
if text.count(needle) != 1:
    raise SystemExit("sensitive profile test anchor mismatch")
text = text.replace(needle, replacement, 1)
path.write_text(text, encoding="utf-8")

ci = Path(".github/workflows/ci.yml")
ci_text = ci.read_text(encoding="utf-8")
marker = "\n  normalize-read-capability:\n"
if marker in ci_text:
    ci_text = ci_text.split(marker, 1)[0].rstrip() + "\n"
ci.write_text(ci_text, encoding="utf-8")
