use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u16 = 1;

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

#[cfg(test)]
mod tests {
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
}
