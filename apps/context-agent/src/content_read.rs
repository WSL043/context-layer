use context_content_vault::{ContentVault, VaultError};
use context_contracts::{LocalTextContent, ReadCapabilityToken, RetrievalClass};
use context_core::content_access::ContentAccessEngine;
use context_storage_sqlite::SqliteRepository;
use uuid::Uuid;

use crate::read_capability::{ReadCapabilityPolicy, environment_read_policy};

const MAX_TEXT_CONTENT_BYTES: usize = 96 * 1024;
const MAX_MEDIA_TYPE_BYTES: usize = 128;

#[derive(Debug, PartialEq, Eq)]
pub enum ContentReadError {
    NotAuthorized,
    Configuration(String),
    UnsupportedContent,
    TooLarge,
    Unavailable,
}

impl ContentReadError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAuthorized => "content_not_authorized",
            Self::Configuration(_) => "read_configuration_invalid",
            Self::UnsupportedContent => "content_type_not_supported",
            Self::TooLarge => "content_too_large",
            Self::Unavailable => "content_unavailable",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotAuthorized => "content read is not authorized".into(),
            Self::Configuration(message) => message.clone(),
            Self::UnsupportedContent => {
                "only uncompressed UTF-8 text/plain local-vault content is readable".into()
            }
            Self::TooLarge => format!(
                "text content exceeds the {MAX_TEXT_CONTENT_BYTES}-byte Local API read limit"
            ),
            Self::Unavailable => {
                "authorized content is unavailable or failed integrity checks".into()
            }
        }
    }
}

pub fn read_text_content_from_environment(
    repository: &SqliteRepository,
    vault: &ContentVault,
    authorization: &ReadCapabilityToken,
    event_id: Uuid,
    sha256: &str,
) -> Result<LocalTextContent, ContentReadError> {
    let policy = match environment_read_policy() {
        Ok(Some(policy)) => policy,
        Ok(None) => return Err(ContentReadError::NotAuthorized),
        Err(error) => return Err(ContentReadError::Configuration(error)),
    };
    read_text_content_with_policy(repository, vault, policy, authorization, event_id, sha256)
}

fn read_text_content_with_policy(
    repository: &SqliteRepository,
    vault: &ContentVault,
    policy: &ReadCapabilityPolicy,
    authorization: &ReadCapabilityToken,
    event_id: Uuid,
    sha256: &str,
) -> Result<LocalTextContent, ContentReadError> {
    let Some(grant) = policy.grant_for_token(authorization) else {
        return Err(ContentReadError::NotAuthorized);
    };
    if !is_lowercase_sha256(sha256) {
        return Err(ContentReadError::NotAuthorized);
    }
    let authorized = ContentAccessEngine::new(repository)
        .authorize_reference(event_id, sha256, grant, |scope| policy.scope_allowed(scope))
        .map_err(|_| ContentReadError::Unavailable)?;
    let Some(authorized) = authorized else {
        return Err(ContentReadError::NotAuthorized);
    };

    let reference = authorized.reference;
    if reference.retrieval_class == RetrievalClass::Secret {
        return Err(ContentReadError::NotAuthorized);
    }
    if reference.storage_class != "local_vault"
        || reference.compression.is_some()
        || reference.media_type.len() > MAX_MEDIA_TYPE_BYTES
        || !is_utf8_plain_text(&reference.media_type)
    {
        return Err(ContentReadError::UnsupportedContent);
    }
    if reference.byte_length > MAX_TEXT_CONTENT_BYTES as u64 {
        return Err(ContentReadError::TooLarge);
    }

    let bytes = match vault.read_verified_bytes(&reference.sha256, MAX_TEXT_CONTENT_BYTES) {
        Ok(bytes) => bytes,
        Err(VaultError::TooLarge { .. }) => return Err(ContentReadError::TooLarge),
        Err(_) => return Err(ContentReadError::Unavailable),
    };
    if bytes.len() as u64 != reference.byte_length {
        return Err(ContentReadError::Unavailable);
    }
    let text = String::from_utf8(bytes).map_err(|_| ContentReadError::Unavailable)?;

    Ok(LocalTextContent {
        event_id,
        sha256: reference.sha256,
        media_type: reference.media_type,
        byte_length: reference.byte_length,
        text,
    })
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_utf8_plain_text(media_type: &str) -> bool {
    let mut parts = media_type.split(';');
    if !parts
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/plain"))
    {
        return false;
    }
    parts.all(|parameter| {
        let parameter = parameter.trim();
        !parameter.is_empty()
            && parameter.split_once('=').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("charset")
                    && value.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
            })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use context_contracts::{ContentRef, EventEnvelopeV2, SensitivityClass};
    use context_core::{ContextEngine, retrieval::RetrievalGrant};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    fn temp_vault() -> (std::path::PathBuf, ContentVault) {
        let root = std::env::temp_dir().join(format!("context-content-read-{}", Uuid::now_v7()));
        let vault = ContentVault::open(&root).unwrap();
        (root, vault)
    }

    fn sensitive_policy(scopes: &[&str]) -> ReadCapabilityPolicy {
        ReadCapabilityPolicy::for_test(
            TOKEN,
            scopes,
            RetrievalGrant {
                max_event_sensitivity: SensitivityClass::Sensitive,
                max_content_retrieval: RetrievalClass::Sensitive,
                include_payload: true,
            },
        )
    }

    fn event_with_ref(reference: ContentRef, scope: &str) -> EventEnvelopeV2 {
        let mut event = EventEnvelopeV2::observed(
            "fixture.content",
            "fixture",
            scope,
            OffsetDateTime::now_utc(),
            json!({"fixture": true}),
            "fixture",
            "content-read fixture",
        );
        event.sensitivity = SensitivityClass::Sensitive;
        event.content_refs.push(reference);
        event
    }

    #[test]
    fn exact_event_and_digest_can_read_verified_sensitive_text() {
        let (root, vault) = temp_vault();
        let stored = vault.put_bytes(b"event-bound personal context").unwrap();
        let reference = ContentRef {
            sha256: stored.sha256.clone(),
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        };
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = event_with_ref(reference, "scope.personal");
        engine.ingest_v2(&event).unwrap();

        let content = read_text_content_with_policy(
            engine.repository(),
            &vault,
            &sensitive_policy(&["scope.personal"]),
            &ReadCapabilityToken(TOKEN.into()),
            event.event_id,
            &stored.sha256,
        )
        .unwrap();
        assert_eq!(content.event_id, event.event_id);
        assert_eq!(content.text, "event-bound personal context");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn digest_from_a_different_event_is_not_authorized() {
        let (root, vault) = temp_vault();
        let first = vault.put_bytes(b"first").unwrap();
        let second = vault.put_bytes(b"second").unwrap();
        let make_ref = |sha256: String, byte_length| ContentRef {
            sha256,
            media_type: "text/plain; charset=utf-8".into(),
            byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        };
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event_a = event_with_ref(
            make_ref(first.sha256.clone(), first.byte_length),
            "scope.personal",
        );
        let event_b = event_with_ref(
            make_ref(second.sha256.clone(), second.byte_length),
            "scope.personal",
        );
        engine.ingest_v2(&event_a).unwrap();
        engine.ingest_v2(&event_b).unwrap();

        let error = read_text_content_with_policy(
            engine.repository(),
            &vault,
            &sensitive_policy(&["scope.personal"]),
            &ReadCapabilityToken(TOKEN.into()),
            event_a.event_id,
            &second.sha256,
        )
        .unwrap_err();
        assert_eq!(error, ContentReadError::NotAuthorized);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unauthorized_scope_secret_ref_and_image_are_not_exposed_as_text() {
        let (root, vault) = temp_vault();
        let stored = vault.put_bytes(b"sensitive bytes").unwrap();
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);

        let secret_ref = ContentRef {
            sha256: stored.sha256.clone(),
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Secret,
        };
        let secret_event = event_with_ref(secret_ref, "scope.personal");
        engine.ingest_v2(&secret_event).unwrap();
        assert_eq!(
            read_text_content_with_policy(
                engine.repository(),
                &vault,
                &sensitive_policy(&["scope.personal"]),
                &ReadCapabilityToken(TOKEN.into()),
                secret_event.event_id,
                &stored.sha256,
            )
            .unwrap_err(),
            ContentReadError::NotAuthorized
        );

        let image_ref = ContentRef {
            sha256: stored.sha256.clone(),
            media_type: "image/png".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        };
        let image_event = event_with_ref(image_ref, "scope.other");
        engine.ingest_v2(&image_event).unwrap();
        assert_eq!(
            read_text_content_with_policy(
                engine.repository(),
                &vault,
                &sensitive_policy(&["scope.personal"]),
                &ReadCapabilityToken(TOKEN.into()),
                image_event.event_id,
                &stored.sha256,
            )
            .unwrap_err(),
            ContentReadError::NotAuthorized
        );

        let image_in_scope = event_with_ref(
            ContentRef {
                sha256: stored.sha256.clone(),
                media_type: "image/png".into(),
                byte_length: stored.byte_length,
                compression: None,
                storage_class: "local_vault".into(),
                retrieval_class: RetrievalClass::Sensitive,
            },
            "scope.personal",
        );
        engine.ingest_v2(&image_in_scope).unwrap();
        assert_eq!(
            read_text_content_with_policy(
                engine.repository(),
                &vault,
                &sensitive_policy(&["scope.personal"]),
                &ReadCapabilityToken(TOKEN.into()),
                image_in_scope.event_id,
                &stored.sha256,
            )
            .unwrap_err(),
            ContentReadError::UnsupportedContent
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vault_tampering_is_unavailable_not_returned() {
        let (root, vault) = temp_vault();
        let stored = vault.put_bytes(b"original text").unwrap();
        let reference = ContentRef {
            sha256: stored.sha256.clone(),
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: stored.byte_length,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: RetrievalClass::Sensitive,
        };
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let event = event_with_ref(reference, "scope.personal");
        engine.ingest_v2(&event).unwrap();
        fs::write(&stored.path, b"tampered text").unwrap();

        assert_eq!(
            read_text_content_with_policy(
                engine.repository(),
                &vault,
                &sensitive_policy(&["scope.personal"]),
                &ReadCapabilityToken(TOKEN.into()),
                event.event_id,
                &stored.sha256,
            )
            .unwrap_err(),
            ContentReadError::Unavailable
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn media_type_accepts_only_plain_text_with_optional_utf8_parameter() {
        assert!(is_utf8_plain_text("text/plain"));
        assert!(is_utf8_plain_text("text/plain; charset=utf-8"));
        assert!(is_utf8_plain_text("TEXT/PLAIN; CHARSET=\"UTF-8\""));
        assert!(!is_utf8_plain_text("text/html; charset=utf-8"));
        assert!(!is_utf8_plain_text("text/plain; charset=utf-16"));
        assert!(!is_utf8_plain_text("text/plain; format=flowed"));
    }
}
