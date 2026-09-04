use anyhow::Result;
use context_content_vault::ContentVault;
use context_contracts::{ContentRef, EventEnvelopeV2, RetrievalClass, SensitivityClass};
use context_platform_windows::ClipboardSnapshot;
use serde_json::json;
use time::OffsetDateTime;

pub fn event_from_snapshot(
    vault: &ContentVault,
    snapshot: ClipboardSnapshot,
    observed_at: OffsetDateTime,
    max_raw_utf16_bytes: usize,
) -> Result<Option<EventEnvelopeV2>> {
    let device_id = std::env::var("COMPUTERNAME").ok();
    let event = match snapshot {
        ClipboardSnapshot::NonText { .. } => return Ok(None),
        ClipboardSnapshot::Text {
            sequence,
            text,
            raw_utf16_bytes,
        } => {
            let stored = vault.put_bytes(text.as_bytes())?;
            let mut event = EventEnvelopeV2::observed(
                "clipboard.text_observed",
                "windows.clipboard",
                "scope.personal",
                observed_at,
                json!({
                    "clipboard_sequence": sequence,
                    "raw_utf16_bytes": raw_utf16_bytes,
                    "text_encoding": "utf-8",
                }),
                "windows-clipboard-v1",
                "CF_UNICODETEXT clipboard snapshot stored by content hash",
            );
            event.content_refs = vec![ContentRef {
                sha256: stored.sha256,
                media_type: "text/plain; charset=utf-8".into(),
                byte_length: stored.byte_length,
                compression: None,
                storage_class: "local_vault".into(),
                retrieval_class: RetrievalClass::Sensitive,
            }];
            event
        }
        ClipboardSnapshot::OversizedText {
            sequence,
            raw_utf16_bytes,
        } => EventEnvelopeV2::observed(
            "clipboard.text_omitted",
            "windows.clipboard",
            "scope.personal",
            observed_at,
            json!({
                "clipboard_sequence": sequence,
                "raw_utf16_bytes": raw_utf16_bytes,
                "raw_utf16_byte_limit": max_raw_utf16_bytes,
                "reason": "capture_size_limit",
            }),
            "windows-clipboard-v1",
            "clipboard text exceeded the bounded raw capture limit",
        ),
    };

    let mut event = event;
    event.device_id = device_id;
    event.sensitivity = SensitivityClass::Sensitive;
    Ok(Some(event))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;

    fn temp_vault_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("context-clipboard-vault-{}", Uuid::now_v7()))
    }

    #[test]
    fn clipboard_body_is_in_vault_but_not_in_raw_event_json() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();
        let body = "clipboard body that must stay out of sqlite json";
        let event = event_from_snapshot(
            &vault,
            ClipboardSnapshot::Text {
                sequence: 42,
                text: body.into(),
                raw_utf16_bytes: (body.encode_utf16().count() * 2 + 2) as u64,
            },
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap()
        .unwrap();

        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains(body));
        assert_eq!(event.content_refs.len(), 1);
        assert_eq!(
            event.content_refs[0].retrieval_class,
            RetrievalClass::Sensitive
        );

        let blob_path = vault
            .path_for_digest(&event.content_refs[0].sha256)
            .unwrap();
        assert_eq!(fs::read_to_string(blob_path).unwrap(), body);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_clipboard_is_explicit_without_a_content_reference() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();
        let event = event_from_snapshot(
            &vault,
            ClipboardSnapshot::OversizedText {
                sequence: 43,
                raw_utf16_bytes: 10_000_000,
            },
            OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap()
        .unwrap();

        assert_eq!(event.event_type, "clipboard.text_omitted");
        assert!(event.content_refs.is_empty());
        assert_eq!(event.payload["reason"], "capture_size_limit");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_text_clipboard_is_out_of_scope_for_this_slice() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();
        assert!(
            event_from_snapshot(
                &vault,
                ClipboardSnapshot::NonText { sequence: 44 },
                OffsetDateTime::from_unix_timestamp(1_700_000_002).unwrap(),
                8 * 1024 * 1024,
            )
            .unwrap()
            .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
