use context_contracts::{
    ContentRef, EventEnvelope, EventEnvelopeV2, EventPayload, ScopeId, SensitivityClass,
};
use thiserror::Error;
use uuid::Uuid;

use crate::retrieval::RetrievalGrant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEventLookup {
    pub event_id: Uuid,
    pub schema_version: u16,
    pub scope_id: ScopeId,
    pub sensitivity: SensitivityClass,
    pub envelope_json: String,
}

pub trait RawEventLookupRepository {
    type Error: std::error::Error + Send + Sync + 'static;

    fn raw_event_by_id(&self, event_id: Uuid) -> Result<Option<RawEventLookup>, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedContentRef {
    pub event_id: Uuid,
    pub scope_id: ScopeId,
    pub event_sensitivity: SensitivityClass,
    pub reference: ContentRef,
}

#[derive(Debug, Error)]
pub enum ContentAccessError<E: std::error::Error + 'static> {
    #[error("repository failed: {0}")]
    Repository(#[source] E),
    #[error("raw event {event_id} uses unsupported schema/envelope version {version}")]
    UnsupportedVersion { event_id: Uuid, version: u16 },
    #[error("raw event {event_id} could not be decoded: {message}")]
    MalformedRawEvent { event_id: Uuid, message: String },
    #[error("raw event {event_id} envelope metadata does not match indexed raw-event columns")]
    EnvelopeMetadataMismatch { event_id: Uuid },
}

pub struct ContentAccessEngine<'a, R> {
    repository: &'a R,
}

impl<'a, R: RawEventLookupRepository> ContentAccessEngine<'a, R> {
    pub fn new(repository: &'a R) -> Self {
        Self { repository }
    }

    /// Returns `Ok(None)` for ordinary authorization misses: unknown event,
    /// unauthorized scope, event sensitivity outside the grant, unreferenced
    /// digest, or reference retrieval class outside the grant. Scope and event
    /// sensitivity are checked from indexed raw-row metadata before parsing the
    /// envelope, so an unauthorized caller cannot distinguish malformed content
    /// in a scope it cannot read.
    pub fn authorize_reference<F>(
        &self,
        event_id: Uuid,
        sha256: &str,
        grant: RetrievalGrant,
        scope_allowed: F,
    ) -> Result<Option<AuthorizedContentRef>, ContentAccessError<R::Error>>
    where
        F: FnOnce(&ScopeId) -> bool,
    {
        let Some(raw) = self
            .repository
            .raw_event_by_id(event_id)
            .map_err(ContentAccessError::Repository)?
        else {
            return Ok(None);
        };

        if !scope_allowed(&raw.scope_id)
            || !event_sensitivity_allowed(raw.sensitivity, grant.max_event_sensitivity)
        {
            return Ok(None);
        }

        let (scope_id, event_sensitivity, refs, envelope_event_id) = match raw.schema_version {
            1 => {
                let envelope: EventEnvelope =
                    serde_json::from_str(&raw.envelope_json).map_err(|error| {
                        ContentAccessError::MalformedRawEvent {
                            event_id,
                            message: error.to_string(),
                        }
                    })?;
                let refs = match envelope.payload {
                    EventPayload::ContentObserved { refs, .. } => refs,
                    _ => Vec::new(),
                };
                (
                    envelope.scope_id,
                    envelope.sensitivity,
                    refs,
                    envelope.event_id,
                )
            }
            2 => {
                let envelope: EventEnvelopeV2 =
                    serde_json::from_str(&raw.envelope_json).map_err(|error| {
                        ContentAccessError::MalformedRawEvent {
                            event_id,
                            message: error.to_string(),
                        }
                    })?;
                (
                    envelope.scope_id,
                    envelope.sensitivity,
                    envelope.content_refs,
                    envelope.event_id,
                )
            }
            version => {
                return Err(ContentAccessError::UnsupportedVersion {
                    event_id,
                    version,
                });
            }
        };

        if envelope_event_id != raw.event_id
            || scope_id != raw.scope_id
            || event_sensitivity != raw.sensitivity
        {
            return Err(ContentAccessError::EnvelopeMetadataMismatch { event_id });
        }

        let Some(reference) = refs.into_iter().find(|reference| reference.sha256 == sha256) else {
            return Ok(None);
        };
        if !retrieval_class_allowed(reference.retrieval_class, grant.max_content_retrieval) {
            return Ok(None);
        }

        Ok(Some(AuthorizedContentRef {
            event_id,
            scope_id,
            event_sensitivity,
            reference,
        }))
    }
}

fn event_sensitivity_allowed(actual: SensitivityClass, maximum: SensitivityClass) -> bool {
    sensitivity_rank(actual) <= sensitivity_rank(maximum)
}

const fn sensitivity_rank(value: SensitivityClass) -> u8 {
    match value {
        SensitivityClass::Metadata => 0,
        SensitivityClass::ContentDerived => 1,
        SensitivityClass::Sensitive => 2,
    }
}

fn retrieval_class_allowed(
    actual: context_contracts::RetrievalClass,
    maximum: context_contracts::RetrievalClass,
) -> bool {
    retrieval_rank(actual) <= retrieval_rank(maximum)
}

const fn retrieval_rank(value: context_contracts::RetrievalClass) -> u8 {
    match value {
        context_contracts::RetrievalClass::Normal => 0,
        context_contracts::RetrievalClass::Sensitive => 1,
        context_contracts::RetrievalClass::Secret => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, convert::Infallible};

    use context_contracts::{EvidenceDescriptor, EvidenceKind, RetrievalClass, SourceId};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    struct FakeRepository {
        rows: HashMap<Uuid, RawEventLookup>,
    }

    impl RawEventLookupRepository for FakeRepository {
        type Error = Infallible;

        fn raw_event_by_id(&self, event_id: Uuid) -> Result<Option<RawEventLookup>, Self::Error> {
            Ok(self.rows.get(&event_id).cloned())
        }
    }

    fn reference(class: RetrievalClass, marker: char) -> ContentRef {
        ContentRef {
            sha256: marker.to_string().repeat(64),
            media_type: "text/plain; charset=utf-8".into(),
            byte_length: 12,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: class,
        }
    }

    fn raw_v2(
        event_id: Uuid,
        sensitivity: SensitivityClass,
        refs: Vec<ContentRef>,
    ) -> RawEventLookup {
        let observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let scope_id = ScopeId("scope.personal".into());
        let event = EventEnvelopeV2 {
            event_id,
            envelope_version: 2,
            event_type: "fixture.content".into(),
            payload_version: 1,
            source: SourceId("fixture".into()),
            source_sequence: None,
            occurred_at: Some(observed_at),
            observed_at,
            ingested_at: observed_at,
            device_id: None,
            scope_id: scope_id.clone(),
            correlation_id: None,
            sensitivity,
            content_refs: refs,
            payload: json!({"fixture": true}),
            evidence: EvidenceDescriptor {
                kind: EvidenceKind::Observed,
                collector: "fixture".into(),
                detail: "fixture".into(),
            },
        };
        RawEventLookup {
            event_id,
            schema_version: 2,
            scope_id,
            sensitivity,
            envelope_json: serde_json::to_string(&event).unwrap(),
        }
    }

    fn sensitive_grant() -> RetrievalGrant {
        RetrievalGrant {
            max_event_sensitivity: SensitivityClass::Sensitive,
            max_content_retrieval: RetrievalClass::Sensitive,
            include_payload: true,
        }
    }

    #[test]
    fn digest_must_be_referenced_by_the_requested_event() {
        let event_a = Uuid::now_v7();
        let event_b = Uuid::now_v7();
        let shared_elsewhere = reference(RetrievalClass::Sensitive, 'b');
        let repository = FakeRepository {
            rows: HashMap::from([
                (
                    event_a,
                    raw_v2(
                        event_a,
                        SensitivityClass::Sensitive,
                        vec![reference(RetrievalClass::Sensitive, 'a')],
                    ),
                ),
                (
                    event_b,
                    raw_v2(
                        event_b,
                        SensitivityClass::Sensitive,
                        vec![shared_elsewhere.clone()],
                    ),
                ),
            ]),
        };
        let access = ContentAccessEngine::new(&repository);

        assert!(
            access
                .authorize_reference(
                    event_a,
                    &shared_elsewhere.sha256,
                    sensitive_grant(),
                    |_| true,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            access
                .authorize_reference(
                    event_b,
                    &shared_elsewhere.sha256,
                    sensitive_grant(),
                    |_| true,
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn metadata_grant_hides_sensitive_event_and_sensitive_grant_hides_secret_ref() {
        let event_id = Uuid::now_v7();
        let secret = reference(RetrievalClass::Secret, 'c');
        let repository = FakeRepository {
            rows: HashMap::from([(
                event_id,
                raw_v2(event_id, SensitivityClass::Sensitive, vec![secret.clone()]),
            )]),
        };
        let access = ContentAccessEngine::new(&repository);

        assert!(
            access
                .authorize_reference(
                    event_id,
                    &secret.sha256,
                    RetrievalGrant::metadata_only(),
                    |_| true,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            access
                .authorize_reference(
                    event_id,
                    &secret.sha256,
                    sensitive_grant(),
                    |_| true,
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unauthorized_scope_is_rejected_before_malformed_envelope_is_parsed() {
        let event_id = Uuid::now_v7();
        let repository = FakeRepository {
            rows: HashMap::from([(
                event_id,
                RawEventLookup {
                    event_id,
                    schema_version: 2,
                    scope_id: ScopeId("scope.other".into()),
                    sensitivity: SensitivityClass::Sensitive,
                    envelope_json: "{ definitely not valid json".into(),
                },
            )]),
        };

        let result = ContentAccessEngine::new(&repository)
            .authorize_reference(event_id, &"d".repeat(64), sensitive_grant(), |scope| {
                scope.0 == "scope.personal"
            })
            .unwrap();
        assert!(result.is_none());
    }
}
