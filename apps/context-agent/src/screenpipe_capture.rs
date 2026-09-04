use anyhow::{Result, ensure};
use context_content_vault::ContentVault;
use context_contracts::{ContentRef, EventEnvelopeV2, RetrievalClass, SensitivityClass};
use context_screenpipe_adapter::ScreenpipeFrame;
use serde_json::{Value, json};
use time::OffsetDateTime;

const MAX_SCREEN_TEXT_BYTES: usize = 8 * 1024 * 1024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenpipeScreenshot {
    Png(Vec<u8>),
    NotFound,
    OmittedTooLarge,
}

pub fn event_from_frame(
    vault: &ContentVault,
    frame: ScreenpipeFrame,
    screenshot: ScreenpipeScreenshot,
    observed_at: OffsetDateTime,
) -> Result<EventEnvelopeV2> {
    let mut refs = Vec::new();
    let mut roles = Vec::<Value>::new();

    let text_status = if frame.text.is_empty() {
        "empty"
    } else if frame.text.len() > MAX_SCREEN_TEXT_BYTES {
        "omitted_too_large"
    } else {
        let stored = vault.put_bytes(frame.text.as_bytes())?;
        let sha256 = stored.sha256;
        roles.push(json!({
            "role": "screen_text",
            "sha256": &sha256,
            "source": frame.text_source.as_str(),
        }));
        refs.push(ContentRef {
            sha256,
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        });
        "retained"
    };

    let screenshot_status = match screenshot {
        ScreenpipeScreenshot::Png(bytes) => {
            ensure!(
                bytes.starts_with(PNG_SIGNATURE),
                "Screenpipe frame body was not a PNG"
            );
            let stored = vault.put_bytes(&bytes)?;
            let sha256 = stored.sha256;
            roles.push(json!({
                "role": "screenshot",
                "sha256": &sha256,
            }));
            refs.push(ContentRef {
                sha256,
                media_type: "image/png".into(),
                byte_length: stored.byte_length,
                compression: None,
                storage_class: "local_vault".into(),
                retrieval_class: RetrievalClass::Sensitive,
            });
            "retained"
        }
        ScreenpipeScreenshot::NotFound => "not_found",
        ScreenpipeScreenshot::OmittedTooLarge => "omitted_too_large",
    };

    let mut event = EventEnvelopeV2::observed(
        "ui.snapshot_observed",
        "screenpipe.local",
        "scope.personal",
        observed_at,
        json!({
            "backend": "screenpipe",
            "screenpipe_frame_id": frame.frame_id,
            "app_name": frame.app_name,
            "window_name": frame.window_name,
            "focused": frame.focused,
            "text_source": frame.text_source.as_str(),
            "text_status": text_status,
            "screenshot_status": screenshot_status,
            "content_roles": roles,
        }),
        "screenpipe-rest-v1",
        "Screenpipe localhost REST search plus frame PNG",
    );
    event.occurred_at = Some(frame.captured_at);
    event.source_sequence = Some(frame.frame_id);
    event.device_id = std::env::var("COMPUTERNAME").ok();
    event.sensitivity = SensitivityClass::Sensitive;
    event.content_refs = refs;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use context_screenpipe_adapter::{ScreenTextSource, ScreenpipeFrame};
    use uuid::Uuid;

    use super::*;

    fn temp_vault() -> (std::path::PathBuf, ContentVault) {
        let root = std::env::temp_dir().join(format!("context-screenpipe-{}", Uuid::now_v7()));
        let vault = ContentVault::open(&root).unwrap();
        (root, vault)
    }

    fn frame(text: &str) -> ScreenpipeFrame {
        ScreenpipeFrame {
            frame_id: 42,
            captured_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            app_name: Some("Example".into()),
            window_name: Some("Example Window".into()),
            text: text.into(),
            text_source: ScreenTextSource::Accessibility,
            focused: Some(true),
        }
    }

    #[test]
    fn raw_text_and_png_live_in_vault_not_event_json() {
        let (root, vault) = temp_vault();
        let body = "private screen text that must stay outside sqlite";
        let png = b"\x89PNG\r\n\x1a\nfixture".to_vec();
        let event = event_from_frame(
            &vault,
            frame(body),
            ScreenpipeScreenshot::Png(png.clone()),
            OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        )
        .unwrap();

        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains(body));
        assert!(!json.contains("fixture"));
        assert_eq!(event.content_refs.len(), 2);
        assert_eq!(event.source_sequence, Some(42));
        assert_eq!(
            event.occurred_at,
            Some(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap())
        );
        for reference in &event.content_refs {
            assert!(vault.path_for_digest(&reference.sha256).unwrap().exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_screenshot_is_explicit_without_losing_text() {
        let (root, vault) = temp_vault();
        let event = event_from_frame(
            &vault,
            frame("screen text"),
            ScreenpipeScreenshot::NotFound,
            OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        )
        .unwrap();
        assert_eq!(event.payload["screenshot_status"], "not_found");
        assert_eq!(event.content_refs.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_png_frame_body_is_rejected_before_cursor_can_advance() {
        let (root, vault) = temp_vault();
        assert!(
            event_from_frame(
                &vault,
                frame("screen text"),
                ScreenpipeScreenshot::Png(b"not a png".to_vec()),
                OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
