use std::{collections::HashSet, sync::OnceLock};

use context_contracts::{
    LocalTimelineCursor, LocalTimelineEntry, LocalTimelinePage, LocalTimelineQuery,
    ReadCapabilityToken, RetrievalClass, ScopeId, SensitivityClass,
};
use context_core::retrieval::{
    RetrievalEngine, RetrievalGrant, TimelineCursor, TimelineEntry, TimelineQuery,
};
use context_storage_sqlite::SqliteRepository;

const READ_TOKEN_ENV: &str = "CONTEXT_LAYER_READ_TOKEN";
const READ_PROFILE_ENV: &str = "CONTEXT_LAYER_READ_PROFILE";
const READ_SCOPES_ENV: &str = "CONTEXT_LAYER_READ_SCOPES";
const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 1024;
const MAX_ALLOWED_SCOPES: usize = 16;
const MAX_SCOPE_BYTES: usize = 256;
const MAX_LOCAL_PAGE_ENTRIES: usize = 20;
const MAX_WIRE_CONTENT_REFS: usize = 32;
const MAX_WIRE_PAYLOAD_BYTES: usize = 32 * 1024;
const MAX_WIRE_PAGE_BYTES: usize = 768 * 1024;

static ENVIRONMENT_POLICY: OnceLock<Result<Option<ReadCapabilityPolicy>, String>> = OnceLock::new();

pub struct ReadCapabilityPolicy {
    token: Box<str>,
    allowed_scopes: HashSet<String>,
    grant: RetrievalGrant,
}

impl ReadCapabilityPolicy {
    fn from_environment() -> Result<Option<Self>, String> {
        let token = std::env::var(READ_TOKEN_ENV).ok();
        let profile = std::env::var(READ_PROFILE_ENV).ok();
        let scopes = std::env::var(READ_SCOPES_ENV).ok();

        let Some(token) = token else {
            if profile.is_some() || scopes.is_some() {
                return Err(format!(
                    "{READ_PROFILE_ENV}/{READ_SCOPES_ENV} require {READ_TOKEN_ENV}"
                ));
            }
            return Ok(None);
        };

        if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&token.len()) {
            return Err(format!(
                "{READ_TOKEN_ENV} must contain {MIN_TOKEN_BYTES} to {MAX_TOKEN_BYTES} bytes"
            ));
        }

        let grant = match profile.as_deref().unwrap_or("metadata") {
            "metadata" => RetrievalGrant::metadata_only(),
            "sensitive" => RetrievalGrant {
                max_event_sensitivity: SensitivityClass::Sensitive,
                max_content_retrieval: RetrievalClass::Sensitive,
                include_payload: true,
            },
            other => {
                return Err(format!(
                    "unsupported {READ_PROFILE_ENV} value {other:?}; expected metadata or sensitive"
                ));
            }
        };

        let scope_values = scopes.unwrap_or_else(|| "scope.personal".into());
        let mut allowed_scopes = HashSet::new();
        for scope in scope_values.split(',').map(str::trim) {
            if scope.is_empty() || scope.len() > MAX_SCOPE_BYTES {
                return Err(format!(
                    "{READ_SCOPES_ENV} contains an empty or oversized scope"
                ));
            }
            allowed_scopes.insert(scope.to_owned());
        }
        if allowed_scopes.is_empty() || allowed_scopes.len() > MAX_ALLOWED_SCOPES {
            return Err(format!(
                "{READ_SCOPES_ENV} must contain 1 to {MAX_ALLOWED_SCOPES} scopes"
            ));
        }

        Ok(Some(Self {
            token: token.into_boxed_str(),
            allowed_scopes,
            grant,
        }))
    }

    #[cfg(test)]
    fn for_test(token: &str, scopes: &[&str], grant: RetrievalGrant) -> Self {
        Self {
            token: token.into(),
            allowed_scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            grant,
        }
    }

    fn authorize(&self, token: &ReadCapabilityToken, scope_id: &ScopeId) -> Option<RetrievalGrant> {
        if !self.allowed_scopes.contains(&scope_id.0) {
            return None;
        }
        constant_time_equal(self.token.as_bytes(), token.0.as_bytes()).then_some(self.grant)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadRequestError {
    NotAuthorized,
    InvalidQuery(String),
    Configuration(String),
    QueryFailed(String),
}

impl ReadRequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAuthorized => "read_not_authorized",
            Self::InvalidQuery(_) => "invalid_timeline_query",
            Self::Configuration(_) => "read_configuration_invalid",
            Self::QueryFailed(_) => "timeline_query_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotAuthorized => "timeline read is not authorized".into(),
            Self::InvalidQuery(message) | Self::Configuration(message) | Self::QueryFailed(message) => {
                message.clone()
            }
        }
    }
}

pub fn query_timeline_from_environment(
    repository: &SqliteRepository,
    authorization: &ReadCapabilityToken,
    query: LocalTimelineQuery,
) -> Result<LocalTimelinePage, ReadRequestError> {
    let policy = ENVIRONMENT_POLICY.get_or_init(ReadCapabilityPolicy::from_environment);
    match policy {
        Ok(Some(policy)) => query_timeline_with_policy(repository, policy, authorization, query),
        Ok(None) => Err(ReadRequestError::NotAuthorized),
        Err(error) => Err(ReadRequestError::Configuration(error.clone())),
    }
}

fn query_timeline_with_policy(
    repository: &SqliteRepository,
    policy: &ReadCapabilityPolicy,
    authorization: &ReadCapabilityToken,
    query: LocalTimelineQuery,
) -> Result<LocalTimelinePage, ReadRequestError> {
    if !(1..=MAX_LOCAL_PAGE_ENTRIES as u16).contains(&query.limit) {
        return Err(ReadRequestError::InvalidQuery(format!(
            "timeline limit must be between 1 and {MAX_LOCAL_PAGE_ENTRIES}"
        )));
    }
    let Some(grant) = policy.authorize(authorization, &query.scope_id) else {
        return Err(ReadRequestError::NotAuthorized);
    };

    let core_page = RetrievalEngine::new(repository)
        .query_timeline(
            &TimelineQuery {
                scope_id: query.scope_id,
                start_at: query.start_at,
                end_at: query.end_at,
                before: query.before.map(|cursor| TimelineCursor {
                    observed_at: cursor.observed_at,
                    event_id: cursor.event_id,
                }),
                limit: query.limit as usize,
            },
            grant,
        )
        .map_err(|error| ReadRequestError::QueryFailed(error.to_string()))?;

    map_wire_page(core_page.entries, core_page.next_cursor, grant)
}

fn map_wire_page(
    entries: Vec<TimelineEntry>,
    core_next_cursor: Option<TimelineCursor>,
    grant: RetrievalGrant,
) -> Result<LocalTimelinePage, ReadRequestError> {
    let mut wire_entries = Vec::with_capacity(entries.len());
    let mut forced_cursor = None;

    for entry in entries {
        let entry_cursor = LocalTimelineCursor {
            observed_at: entry.observed_at,
            event_id: entry.event_id,
        };
        let wire_entry = map_wire_entry(entry, grant)?;
        wire_entries.push(wire_entry);

        let candidate = LocalTimelinePage {
            entries: wire_entries.clone(),
            next_cursor: None,
        };
        let size = serde_json::to_vec(&candidate)
            .map_err(|error| ReadRequestError::QueryFailed(error.to_string()))?
            .len();
        if size > MAX_WIRE_PAGE_BYTES {
            wire_entries.pop();
            if wire_entries.is_empty() {
                return Err(ReadRequestError::QueryFailed(
                    "a timeline entry exceeds the bounded local API response budget".into(),
                ));
            }
            forced_cursor = wire_entries.last().map(|entry| LocalTimelineCursor {
                observed_at: entry.observed_at,
                event_id: entry.event_id,
            });
            break;
        }
        forced_cursor = Some(entry_cursor);
    }

    let next_cursor = if wire_entries.len() < entries_len_hint(&wire_entries, &forced_cursor) {
        forced_cursor
    } else {
        core_next_cursor.map(|cursor| LocalTimelineCursor {
            observed_at: cursor.observed_at,
            event_id: cursor.event_id,
        })
    };

    Ok(LocalTimelinePage {
        entries: wire_entries,
        next_cursor,
    })
}

// Keeps the page mapping branch explicit without exposing an unbounded page-size calculation.
fn entries_len_hint(entries: &[LocalTimelineEntry], cursor: &Option<LocalTimelineCursor>) -> usize {
    entries.len() + usize::from(cursor.is_some())
}

fn map_wire_entry(
    entry: TimelineEntry,
    grant: RetrievalGrant,
) -> Result<LocalTimelineEntry, ReadRequestError> {
    let total_refs = entry.content_refs.len();
    let mut content_refs = entry.content_refs;
    content_refs.truncate(MAX_WIRE_CONTENT_REFS);
    let omitted_refs = total_refs.saturating_sub(content_refs.len());

    let mut payload = entry.payload;
    let mut payload_omitted_reason = if payload.is_none() {
        Some(if grant.include_payload {
            "policy"
        } else {
            "profile"
        }
        .to_owned())
    } else {
        None
    };

    if omitted_refs > 0 {
        payload = None;
        payload_omitted_reason = Some("content_ref_wire_limit".into());
    } else if let Some(value) = payload.as_ref() {
        let payload_size = serde_json::to_vec(value)
            .map_err(|error| ReadRequestError::QueryFailed(error.to_string()))?
            .len();
        if payload_size > MAX_WIRE_PAYLOAD_BYTES {
            payload = None;
            payload_omitted_reason = Some("payload_wire_limit".into());
        }
    }

    Ok(LocalTimelineEntry {
        event_id: entry.event_id,
        schema_version: entry.schema_version,
        event_type: entry.event_type,
        source: entry.source,
        source_sequence: entry.source_sequence,
        occurred_at: entry.occurred_at,
        observed_at: entry.observed_at,
        ingested_at: entry.ingested_at,
        device_id: entry.device_id,
        scope_id: entry.scope_id,
        correlation_id: entry.correlation_id,
        sensitivity: entry.sensitivity,
        content_refs,
        content_refs_omitted: u32::try_from(omitted_refs).unwrap_or(u32::MAX),
        payload,
        payload_omitted_reason,
        evidence: entry.evidence,
    })
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use context_contracts::{EventEnvelopeV2, SensitivityClass};
    use context_core::{ContextEngine, retrieval::RetrievalGrant};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    fn query() -> LocalTimelineQuery {
        LocalTimelineQuery {
            scope_id: ScopeId("scope.personal".into()),
            start_at: None,
            end_at: None,
            before: None,
            limit: 10,
        }
    }

    #[test]
    fn token_and_scope_are_both_required() {
        let policy = ReadCapabilityPolicy::for_test(
            TOKEN,
            &["scope.personal"],
            RetrievalGrant::metadata_only(),
        );
        assert!(policy
            .authorize(&ReadCapabilityToken(TOKEN.into()), &ScopeId("scope.personal".into()))
            .is_some());
        assert!(policy
            .authorize(&ReadCapabilityToken("wrong-token-that-is-long-enough-xxxxxxxx".into()), &ScopeId("scope.personal".into()))
            .is_none());
        assert!(policy
            .authorize(&ReadCapabilityToken(TOKEN.into()), &ScopeId("scope.other".into()))
            .is_none());
    }

    #[test]
    fn metadata_profile_cannot_read_sensitive_event() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut event = EventEnvelopeV2::observed(
            "browser.page_state",
            "browser.chromium",
            "scope.personal",
            OffsetDateTime::now_utc(),
            json!({"url": "https://private.test"}),
            "fixture",
            "fixture",
        );
        event.sensitivity = SensitivityClass::Sensitive;
        engine.ingest_v2(&event).unwrap();

        let policy = ReadCapabilityPolicy::for_test(
            TOKEN,
            &["scope.personal"],
            RetrievalGrant::metadata_only(),
        );
        let page = query_timeline_with_policy(
            engine.repository(),
            &policy,
            &ReadCapabilityToken(TOKEN.into()),
            query(),
        )
        .unwrap();
        assert!(page.entries.is_empty());
    }

    #[test]
    fn sensitive_profile_can_return_small_sensitive_payload() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut event = EventEnvelopeV2::observed(
            "browser.page_state",
            "browser.chromium",
            "scope.personal",
            OffsetDateTime::now_utc(),
            json!({"url": "https://private.test"}),
            "fixture",
            "fixture",
        );
        event.sensitivity = SensitivityClass::Sensitive;
        engine.ingest_v2(&event).unwrap();

        let policy = ReadCapabilityPolicy::for_test(
            TOKEN,
            &["scope.personal"],
            RetrievalGrant {
                max_event_sensitivity: SensitivityClass::Sensitive,
                max_content_retrieval: RetrievalClass::Sensitive,
                include_payload: true,
            },
        );
        let page = query_timeline_with_policy(
            engine.repository(),
            &policy,
            &ReadCapabilityToken(TOKEN.into()),
            query(),
        )
        .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.entries[0].payload.as_ref().unwrap()["url"],
            "https://private.test"
        );
    }

    #[test]
    fn oversized_payload_is_explicitly_omitted_from_wire_page() {
        let repository = SqliteRepository::in_memory().unwrap();
        let mut engine = ContextEngine::new(repository);
        let mut event = EventEnvelopeV2::observed(
            "future.large_payload",
            "fixture",
            "scope.personal",
            OffsetDateTime::now_utc(),
            json!({"body": "x".repeat(MAX_WIRE_PAYLOAD_BYTES + 1)}),
            "fixture",
            "fixture",
        );
        event.sensitivity = SensitivityClass::Sensitive;
        engine.ingest_v2(&event).unwrap();

        let policy = ReadCapabilityPolicy::for_test(
            TOKEN,
            &["scope.personal"],
            RetrievalGrant {
                max_event_sensitivity: SensitivityClass::Sensitive,
                max_content_retrieval: RetrievalClass::Sensitive,
                include_payload: true,
            },
        );
        let page = query_timeline_with_policy(
            engine.repository(),
            &policy,
            &ReadCapabilityToken(TOKEN.into()),
            query(),
        )
        .unwrap();
        assert!(page.entries[0].payload.is_none());
        assert_eq!(
            page.entries[0].payload_omitted_reason.as_deref(),
            Some("payload_wire_limit")
        );
    }
}
