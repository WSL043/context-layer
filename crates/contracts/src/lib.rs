use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u16 = 1;
pub const CURRENT_ENVELOPE_VERSION: u16 = 2;
pub const LOCAL_API_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub provider: String,
    pub namespace: String,
    pub opaque_id: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Observed,
    Inferred,
    UserConfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceDescriptor {
    pub kind: EvidenceKind,
    pub collector: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityClass {
    Metadata,
    ContentDerived,
    Sensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalClass {
    Normal,
    Sensitive,
    Secret,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRef {
    pub sha256: String,
    pub media_type: String,
    pub byte_length: u64,
    pub compression: Option<String>,
    pub storage_class: String,
    pub retrieval_class: RetrievalClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChange {
    Created,
    Modified,
    Renamed,
    Moved,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    TaskStarted {
        task_id: Uuid,
        name: String,
    },
    FileObserved {
        identity: FileIdentity,
        path: String,
        change: FileChange,
    },
    BrowserDownloadObserved {
        download_id: Uuid,
        url: String,
        referrer: Option<String>,
        final_path: String,
    },
    ContentObserved {
        content_kind: String,
        refs: Vec<ContentRef>,
    },
    DownloadMatched {
        download_id: Uuid,
        identity: FileIdentity,
        url: String,
        source_event_ids: Vec<Uuid>,
    },
    CollectorGap {
        collector: String,
        last_sequence: Option<u64>,
        reason: String,
    },
}

impl EventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::TaskStarted { .. } => "task.started",
            Self::FileObserved { .. } => "file.observed",
            Self::BrowserDownloadObserved { .. } => "browser.download_observed",
            Self::ContentObserved { .. } => "content.observed",
            Self::DownloadMatched { .. } => "download.matched",
            Self::CollectorGap { .. } => "collector.gap",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: Uuid,
    pub schema_version: u16,
    pub source: SourceId,
    pub source_sequence: Option<u64>,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ingested_at: OffsetDateTime,
    pub scope_id: ScopeId,
    pub correlation_id: Option<Uuid>,
    pub sensitivity: SensitivityClass,
    pub payload: EventPayload,
    pub evidence: EvidenceDescriptor,
}

impl EventEnvelope {
    pub fn observed(
        source: impl Into<String>,
        scope_id: impl Into<String>,
        observed_at: OffsetDateTime,
        payload: EventPayload,
        collector: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            schema_version: CURRENT_SCHEMA_VERSION,
            source: SourceId(source.into()),
            source_sequence: None,
            observed_at,
            ingested_at: OffsetDateTime::now_utc(),
            scope_id: ScopeId(scope_id.into()),
            correlation_id: None,
            sensitivity: SensitivityClass::Metadata,
            payload,
            evidence: EvidenceDescriptor {
                kind: EvidenceKind::Observed,
                collector: collector.into(),
                detail: detail.into(),
            },
        }
    }
}

/// Stable v2 ingest envelope for collectors whose payloads evolve independently
/// from the Context Layer binary. The core may not understand `event_type` or
/// `payload_version`, but it can still retain the payload as raw evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelopeV2 {
    pub event_id: Uuid,
    pub envelope_version: u16,
    pub event_type: String,
    pub payload_version: u16,
    pub source: SourceId,
    pub source_sequence: Option<u64>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub occurred_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ingested_at: OffsetDateTime,
    pub device_id: Option<String>,
    pub scope_id: ScopeId,
    pub correlation_id: Option<Uuid>,
    pub sensitivity: SensitivityClass,
    #[serde(default)]
    pub content_refs: Vec<ContentRef>,
    pub payload: Value,
    pub evidence: EvidenceDescriptor,
}

impl EventEnvelopeV2 {
    /// Creates a live observed event at payload version 1. Collectors that emit a
    /// later payload version set `payload_version` explicitly after construction.
    pub fn observed(
        event_type: impl Into<String>,
        source: impl Into<String>,
        scope_id: impl Into<String>,
        observed_at: OffsetDateTime,
        payload: Value,
        collector: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            envelope_version: CURRENT_ENVELOPE_VERSION,
            event_type: event_type.into(),
            payload_version: 1,
            source: SourceId(source.into()),
            source_sequence: None,
            occurred_at: None,
            observed_at,
            ingested_at: OffsetDateTime::now_utc(),
            device_id: None,
            scope_id: ScopeId(scope_id.into()),
            correlation_id: None,
            sensitivity: SensitivityClass::Metadata,
            content_refs: Vec::new(),
            payload,
            evidence: EvidenceDescriptor {
                kind: EvidenceKind::Observed,
                collector: collector.into(),
                detail: detail.into(),
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReadCapabilityToken(pub String);

impl fmt::Debug for ReadCapabilityToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReadCapabilityToken([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalTimelineCursor {
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    pub event_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalTimelineQuery {
    pub scope_id: ScopeId,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub start_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub end_at: Option<OffsetDateTime>,
    pub before: Option<LocalTimelineCursor>,
    pub limit: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalTimelineEntry {
    pub event_id: Uuid,
    pub schema_version: u16,
    pub event_type: String,
    pub source: SourceId,
    pub source_sequence: Option<u64>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub occurred_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ingested_at: OffsetDateTime,
    pub device_id: Option<String>,
    pub scope_id: ScopeId,
    pub correlation_id: Option<Uuid>,
    pub sensitivity: SensitivityClass,
    pub content_refs: Vec<ContentRef>,
    pub content_refs_omitted: u32,
    pub payload: Option<Value>,
    pub payload_omitted_reason: Option<String>,
    pub evidence: EvidenceDescriptor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalTimelinePage {
    pub entries: Vec<LocalTimelineEntry>,
    pub next_cursor: Option<LocalTimelineCursor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalApiRequest {
    pub request_id: Uuid,
    pub protocol_version: u16,
    pub command: LocalApiCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalApiCommand {
    Handshake {
        client_name: String,
    },
    SubmitEvent {
        event: Box<EventEnvelope>,
    },
    SubmitEventV2 {
        event: Box<EventEnvelopeV2>,
    },
    QueryTimeline {
        authorization: ReadCapabilityToken,
        query: LocalTimelineQuery,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalApiResponse {
    pub request_id: Uuid,
    pub protocol_version: u16,
    pub result: LocalApiResult,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalApiResult {
    Ready { server_name: String },
    EventAccepted { event_id: Uuid, duplicate: bool },
    TimelinePage { page: LocalTimelinePage },
    Error { code: String, message: String },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn event_contract_round_trips_without_losing_evidence() {
        let event = EventEnvelope::observed(
            "browser.edge",
            "scope.downloads",
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            EventPayload::BrowserDownloadObserved {
                download_id: Uuid::now_v7(),
                url: "https://example.test/report.pdf".into(),
                referrer: Some("https://example.test/".into()),
                final_path: r"C:\Users\Example\Downloads\report.pdf".into(),
            },
            "edge-extension",
            "downloads API",
        );

        let json = serde_json::to_string(&event).unwrap();
        let decoded: EventEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, event);
        assert!(json.contains("browser_download_observed"));
        assert!(json.contains("downloads API"));
    }

    #[test]
    fn content_reference_round_trips_without_embedding_blob_bytes() {
        let event = EventEnvelope::observed(
            "screenpipe.local",
            "scope.personal",
            OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
            EventPayload::ContentObserved {
                content_kind: "ui.screenshot".into(),
                refs: vec![ContentRef {
                    sha256: "a".repeat(64),
                    media_type: "image/png".into(),
                    byte_length: 1024,
                    compression: None,
                    storage_class: "local_vault".into(),
                    retrieval_class: RetrievalClass::Sensitive,
                }],
            },
            "screenpipe-adapter",
            "content-addressed screenshot reference",
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("content_observed"));
        assert!(json.contains("local_vault"));
        assert!(!json.contains("PNG"));
        let decoded: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn v2_keeps_unknown_event_types_and_future_payload_versions() {
        let occurred_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let observed_at = OffsetDateTime::from_unix_timestamp(1_700_000_120).unwrap();
        let mut event = EventEnvelopeV2::observed(
            "wechat.message",
            "wechat.ui-parser",
            "scope.personal",
            observed_at,
            json!({
                "conversation": "supplier-a",
                "text": "sample ships Monday",
                "future_field": {"nested": true}
            }),
            "wechat-parser-v0",
            "opaque payload fixture",
        );
        event.payload_version = 99;
        event.occurred_at = Some(occurred_at);
        event.device_id = Some("desktop-primary".into());
        event.sensitivity = SensitivityClass::Sensitive;

        let encoded = serde_json::to_string(&event).unwrap();
        let decoded: EventEnvelopeV2 = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, event);
        assert_eq!(decoded.payload_version, 99);
        assert_eq!(decoded.occurred_at, Some(occurred_at));
        assert_eq!(decoded.observed_at, observed_at);
        assert_eq!(decoded.payload["future_field"]["nested"], true);
    }

    #[test]
    fn checked_in_v1_fixture_remains_readable_and_stable() {
        let fixture = include_str!("../../../schemas/events/v1/browser_download_observed.json");
        let decoded: EventEnvelope = serde_json::from_str(fixture).unwrap();

        assert_eq!(decoded.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(matches!(
            decoded.payload,
            EventPayload::BrowserDownloadObserved { .. }
        ));
        assert_eq!(decoded.evidence.kind, EvidenceKind::Observed);

        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let actual = serde_json::to_value(decoded).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn checked_in_v2_fixture_preserves_opaque_payload() {
        let fixture = include_str!("../../../schemas/events/v2/unknown_payload.json");
        let decoded: EventEnvelopeV2 = serde_json::from_str(fixture).unwrap();

        assert_eq!(decoded.envelope_version, CURRENT_ENVELOPE_VERSION);
        assert_eq!(decoded.event_type, "future.collector.observation");
        assert_eq!(decoded.payload_version, 42);
        assert_eq!(decoded.payload["unknown_field"], "must survive");
    }

    #[test]
    fn local_api_request_has_an_explicit_protocol_version() {
        let request = LocalApiRequest {
            request_id: Uuid::now_v7(),
            protocol_version: LOCAL_API_VERSION,
            command: LocalApiCommand::Handshake {
                client_name: "contract-test".into(),
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        let decoded: LocalApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
        assert!(json.contains("protocol_version"));
    }

    #[test]
    fn local_api_can_carry_v2_events() {
        let event = EventEnvelopeV2::observed(
            "ui.window_focused",
            "windows.foreground",
            "scope.personal",
            OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            json!({"process": "notepad.exe"}),
            "foreground-window-v1",
            "Win32 foreground window",
        );
        let request = LocalApiRequest {
            request_id: Uuid::now_v7(),
            protocol_version: LOCAL_API_VERSION,
            command: LocalApiCommand::SubmitEventV2 {
                event: Box::new(event),
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("submit_event_v2"));
        let decoded: LocalApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn timeline_query_token_is_redacted_in_debug_and_carries_no_grant_fields() {
        let request = LocalApiRequest {
            request_id: Uuid::now_v7(),
            protocol_version: LOCAL_API_VERSION,
            command: LocalApiCommand::QueryTimeline {
                authorization: ReadCapabilityToken(
                    "this-is-a-test-token-with-more-than-32-bytes".into(),
                ),
                query: LocalTimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: None,
                    limit: 10,
                },
            },
        };

        let debug = format!("{request:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("this-is-a-test-token"));

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("query_timeline"));
        assert!(!json.contains("max_event_sensitivity"));
        assert!(!json.contains("max_content_retrieval"));
        let decoded: LocalApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }
}
