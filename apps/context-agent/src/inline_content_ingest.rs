use std::collections::HashSet;

use context_content_vault::ContentVault;
use context_contracts::{
    ContentRef, EventEnvelopeV2, LocalSensitiveTextPart, RetrievalClass, SensitivityClass,
};
use context_core::{ContextEngine, IngestOutcome};
use context_storage_sqlite::SqliteRepository;
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_INLINE_PARTS: usize = 4;
const MAX_INLINE_PART_BYTES: usize = 64 * 1024;
const MAX_INLINE_TOTAL_BYTES: usize = 96 * 1024;
const MAX_ROLE_BYTES: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum InlineContentError {
    VaultUnavailable,
    InvalidEvent(String),
    InvalidPart(String),
    Vault(String),
    Ingest(String),
}

impl InlineContentError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::VaultUnavailable => "content_vault_unavailable",
            Self::InvalidEvent(_) => "invalid_inline_content_event",
            Self::InvalidPart(_) => "invalid_inline_content_part",
            Self::Vault(_) => "content_vault_failed",
            Self::Ingest(_) => "ingest_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::VaultUnavailable => "content vault is unavailable in this runtime".into(),
            Self::InvalidEvent(message)
            | Self::InvalidPart(message)
            | Self::Vault(message)
            | Self::Ingest(message) => message.clone(),
        }
    }
}

pub fn ingest_sensitive_text_event(
    engine: &mut ContextEngine<SqliteRepository>,
    vault: Option<&ContentVault>,
    mut event: EventEnvelopeV2,
    parts: Vec<LocalSensitiveTextPart>,
) -> Result<(Uuid, bool), InlineContentError> {
    let vault = vault.ok_or(InlineContentError::VaultUnavailable)?;
    validate_event(&event)?;
    validate_parts(&parts)?;

    let payload = event
        .payload
        .as_object_mut()
        .ok_or_else(|| InlineContentError::InvalidEvent("payload must be a JSON object".into()))?;
    if payload.contains_key("content_roles") {
        return Err(InlineContentError::InvalidEvent(
            "payload must not predefine content_roles".into(),
        ));
    }

    event.sensitivity = SensitivityClass::Sensitive;
    event.content_refs.clear();
    let mut roles = Vec::with_capacity(parts.len());
    for part in parts {
        let stored = vault
            .put_bytes(part.text.as_bytes())
            .map_err(|error| InlineContentError::Vault(error.to_string()))?;
        roles.push(json!({
            "role": part.role,
            "sha256": &stored.sha256,
        }));
        event.content_refs.push(ContentRef {
            sha256: stored.sha256,
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        });
    }
    payload.insert("content_roles".into(), Value::Array(roles));

    let event_id = event.event_id;
    let report = engine
        .ingest_v2(&event)
        .map_err(|error| InlineContentError::Ingest(error.to_string()))?;
    Ok((event_id, report.outcome == IngestOutcome::Duplicate))
}

fn validate_event(event: &EventEnvelopeV2) -> Result<(), InlineContentError> {
    if !event.content_refs.is_empty() {
        return Err(InlineContentError::InvalidEvent(
            "client-supplied content_refs are not accepted by inline content ingest".into(),
        ));
    }
    if event.event_type.trim().is_empty() {
        return Err(InlineContentError::InvalidEvent(
            "event_type must not be empty".into(),
        ));
    }
    if event.payload_version == 0 {
        return Err(InlineContentError::InvalidEvent(
            "payload_version must be greater than zero".into(),
        ));
    }
    if !event.payload.is_object() {
        return Err(InlineContentError::InvalidEvent(
            "payload must be a JSON object".into(),
        ));
    }
    Ok(())
}

fn validate_parts(parts: &[LocalSensitiveTextPart]) -> Result<(), InlineContentError> {
    if parts.len() > MAX_INLINE_PARTS {
        return Err(InlineContentError::InvalidPart(format!(
            "at most {MAX_INLINE_PARTS} inline text parts are accepted"
        )));
    }

    let mut seen_roles = HashSet::new();
    let mut total = 0usize;
    for part in parts {
        if !valid_role(&part.role) {
            return Err(InlineContentError::InvalidPart(
                "part role must contain 1 to 32 lowercase ASCII letters, digits, '.', '_' or '-'"
                    .into(),
            ));
        }
        if !seen_roles.insert(part.role.as_str()) {
            return Err(InlineContentError::InvalidPart(
                "inline text part roles must be unique".into(),
            ));
        }
        let length = part.text.len();
        if length == 0 || length > MAX_INLINE_PART_BYTES {
            return Err(InlineContentError::InvalidPart(format!(
                "each inline text part must contain 1 to {MAX_INLINE_PART_BYTES} UTF-8 bytes"
            )));
        }
        total = total.saturating_add(length);
        if total > MAX_INLINE_TOTAL_BYTES {
            return Err(InlineContentError::InvalidPart(format!(
                "inline text parts exceed the {MAX_INLINE_TOTAL_BYTES}-byte total limit"
            )));
        }
    }
    Ok(())
}

fn valid_role(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= MAX_ROLE_BYTES
        && role.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use context_contracts::{EvidenceDescriptor, EvidenceKind, ScopeId, SourceId};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    fn temp_vault() -> (std::path::PathBuf, ContentVault) {
        let root = std::env::temp_dir().join(format!("context-inline-content-{}", Uuid::now_v7()));
        let vault = ContentVault::open(&root).unwrap();
        (root, vault)
    }

    fn event() -> EventEnvelopeV2 {
        EventEnvelopeV2 {
            event_id: Uuid::now_v7(),
            envelope_version: 2,
            event_type: "browser.copy_observed".into(),
            payload_version: 1,
            source: SourceId("browser.chromium".into()),
            source_sequence: Some(10),
            occurred_at: None,
            observed_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            ingested_at: OffsetDateTime::UNIX_EPOCH,
            device_id: None,
            scope_id: ScopeId("scope.personal".into()),
            correlation_id: None,
            sensitivity: SensitivityClass::Metadata,
            content_refs: Vec::new(),
            payload: json!({"interaction": "copy"}),
            evidence: EvidenceDescriptor {
                kind: EvidenceKind::Observed,
                collector: "fixture".into(),
                detail: "fixture".into(),
            },
        }
    }

    #[test]
    fn stores_raw_text_only_in_vault_and_forces_sensitive_refs() {
        let (root, vault) = temp_vault();
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let body = "selected private browser text";
        let event = event();
        let event_id = event.event_id;
        ingest_sensitive_text_event(
            &mut engine,
            Some(&vault),
            event,
            vec![LocalSensitiveTextPart {
                role: "selected_text".into(),
                text: body.into(),
            }],
        )
        .unwrap();

        let raw = engine
            .repository()
            .raw_event_envelope_json(event_id)
            .unwrap()
            .unwrap();
        assert!(!raw.contains(body));
        let stored: EventEnvelopeV2 = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored.sensitivity, SensitivityClass::Sensitive);
        assert_eq!(stored.content_refs.len(), 1);
        assert_eq!(stored.content_refs[0].retrieval_class, RetrievalClass::Sensitive);
        assert_eq!(stored.payload["content_roles"][0]["role"], "selected_text");
        assert!(vault
            .path_for_digest(&stored.content_refs[0].sha256)
            .unwrap()
            .exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zero_parts_keeps_explicit_metadata_only_observation() {
        let (root, vault) = temp_vault();
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = event();
        let event_id = event.event_id;
        ingest_sensitive_text_event(&mut engine, Some(&vault), event, Vec::new()).unwrap();

        let raw = engine
            .repository()
            .raw_event_envelope_json(event_id)
            .unwrap()
            .unwrap();
        let stored: EventEnvelopeV2 = serde_json::from_str(&raw).unwrap();
        assert!(stored.content_refs.is_empty());
        assert_eq!(stored.payload["content_roles"], json!([]));
        assert_eq!(stored.sensitivity, SensitivityClass::Sensitive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn client_refs_duplicate_roles_and_oversized_parts_are_rejected() {
        let (root, vault) = temp_vault();
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);

        let mut with_ref = event();
        with_ref.content_refs.push(ContentRef {
            sha256: "a".repeat(64),
            media_type: "text/plain".into(),
            byte_length: 1,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Normal,
        });
        assert!(matches!(
            ingest_sensitive_text_event(&mut engine, Some(&vault), with_ref, Vec::new()),
            Err(InlineContentError::InvalidEvent(_))
        ));

        let duplicate = vec![
            LocalSensitiveTextPart {
                role: "selected_text".into(),
                text: "one".into(),
            },
            LocalSensitiveTextPart {
                role: "selected_text".into(),
                text: "two".into(),
            },
        ];
        assert!(matches!(
            ingest_sensitive_text_event(&mut engine, Some(&vault), event(), duplicate),
            Err(InlineContentError::InvalidPart(_))
        ));

        assert!(matches!(
            ingest_sensitive_text_event(
                &mut engine,
                Some(&vault),
                event(),
                vec![LocalSensitiveTextPart {
                    role: "selected_text".into(),
                    text: "x".repeat(MAX_INLINE_PART_BYTES + 1),
                }],
            ),
            Err(InlineContentError::InvalidPart(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
