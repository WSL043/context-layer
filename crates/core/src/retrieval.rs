use context_contracts::{
    ContentRef, EventEnvelope, EventEnvelopeV2, EventPayload, EvidenceDescriptor, RetrievalClass,
    ScopeId, SensitivityClass, SourceId,
};
use serde_json::Value;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

const MAX_TIMELINE_PAGE_SIZE: usize = 200;
const RAW_BATCH_SIZE: usize = 200;
const MAX_RAW_SCAN_PER_QUERY: usize = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineCursor {
    pub observed_at: OffsetDateTime,
    pub event_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineQuery {
    pub scope_id: ScopeId,
    pub start_at: Option<OffsetDateTime>,
    pub end_at: Option<OffsetDateTime>,
    pub before: Option<TimelineCursor>,
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetrievalGrant {
    pub max_event_sensitivity: SensitivityClass,
    pub max_content_retrieval: RetrievalClass,
    pub include_payload: bool,
}

impl RetrievalGrant {
    pub const fn metadata_only() -> Self {
        Self {
            max_event_sensitivity: SensitivityClass::Metadata,
            max_content_retrieval: RetrievalClass::Normal,
            include_payload: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineEntry {
    pub event_id: Uuid,
    pub schema_version: u16,
    pub event_type: String,
    pub source: SourceId,
    pub source_sequence: Option<u64>,
    pub occurred_at: Option<OffsetDateTime>,
    pub observed_at: OffsetDateTime,
    pub ingested_at: OffsetDateTime,
    pub device_id: Option<String>,
    pub scope_id: ScopeId,
    pub correlation_id: Option<Uuid>,
    pub sensitivity: SensitivityClass,
    pub content_refs: Vec<ContentRef>,
    pub payload: Option<Value>,
    pub evidence: EvidenceDescriptor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelinePage {
    pub entries: Vec<TimelineEntry>,
    pub next_cursor: Option<TimelineCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTimelineQuery {
    pub scope_id: ScopeId,
    pub start_at: Option<OffsetDateTime>,
    pub end_at: Option<OffsetDateTime>,
    pub before: Option<TimelineCursor>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTimelineRecord {
    pub event_id: Uuid,
    pub schema_version: u16,
    pub observed_at: OffsetDateTime,
    pub envelope_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTimelinePage {
    pub records: Vec<RawTimelineRecord>,
    pub has_more: bool,
}

pub trait TimelineRepository {
    type Error: std::error::Error + Send + Sync + 'static;

    fn query_raw_timeline(&self, query: &RawTimelineQuery) -> Result<RawTimelinePage, Self::Error>;
}

#[derive(Debug, Error)]
pub enum RetrievalError<E: std::error::Error + 'static> {
    #[error("timeline scope must not be empty")]
    EmptyScope,
    #[error("timeline limit must be between 1 and {MAX_TIMELINE_PAGE_SIZE}")]
    InvalidLimit,
    #[error("timeline start must be earlier than end")]
    InvalidTimeRange,
    #[error("repository failed: {0}")]
    Repository(#[source] E),
    #[error("raw event {event_id} uses unsupported schema/envelope version {version}")]
    UnsupportedVersion { event_id: Uuid, version: u16 },
    #[error("raw event {event_id} could not be decoded: {message}")]
    MalformedRawEvent { event_id: Uuid, message: String },
}

pub struct RetrievalEngine<'a, R> {
    repository: &'a R,
}

impl<'a, R: TimelineRepository> RetrievalEngine<'a, R> {
    pub fn new(repository: &'a R) -> Self {
        Self { repository }
    }

    pub fn query_timeline(
        &self,
        query: &TimelineQuery,
        grant: RetrievalGrant,
    ) -> Result<TimelinePage, RetrievalError<R::Error>> {
        validate_query(query)?;

        let mut entries = Vec::with_capacity(query.limit);
        let mut raw_cursor = query.before.clone();
        let mut scanned = 0usize;
        let mut next_cursor = None;

        while entries.len() < query.limit && scanned < MAX_RAW_SCAN_PER_QUERY {
            let remaining_scan = MAX_RAW_SCAN_PER_QUERY - scanned;
            let raw_limit = RAW_BATCH_SIZE.min(remaining_scan);
            let raw_page = self
                .repository
                .query_raw_timeline(&RawTimelineQuery {
                    scope_id: query.scope_id.clone(),
                    start_at: query.start_at,
                    end_at: query.end_at,
                    before: raw_cursor.clone(),
                    limit: raw_limit,
                })
                .map_err(RetrievalError::Repository)?;

            if raw_page.records.is_empty() {
                next_cursor = None;
                break;
            }

            let record_count = raw_page.records.len();
            let has_more_after_page = raw_page.has_more;
            for (index, record) in raw_page.records.into_iter().enumerate() {
                scanned += 1;
                let cursor = TimelineCursor {
                    observed_at: record.observed_at,
                    event_id: record.event_id,
                };
                raw_cursor = Some(cursor.clone());

                if let Some(entry) = decode_visible_entry(record, grant)? {
                    entries.push(entry);
                }

                if entries.len() == query.limit {
                    next_cursor = if index + 1 < record_count || has_more_after_page {
                        Some(cursor)
                    } else {
                        None
                    };
                    return Ok(TimelinePage {
                        entries,
                        next_cursor,
                    });
                }
            }

            if !has_more_after_page {
                next_cursor = None;
                break;
            }
            next_cursor = raw_cursor.clone();
        }

        if scanned >= MAX_RAW_SCAN_PER_QUERY && entries.len() < query.limit {
            next_cursor = raw_cursor;
        }

        Ok(TimelinePage {
            entries,
            next_cursor,
        })
    }
}

fn validate_query<E: std::error::Error + 'static>(
    query: &TimelineQuery,
) -> Result<(), RetrievalError<E>> {
    if query.scope_id.0.trim().is_empty() {
        return Err(RetrievalError::EmptyScope);
    }
    if !(1..=MAX_TIMELINE_PAGE_SIZE).contains(&query.limit) {
        return Err(RetrievalError::InvalidLimit);
    }
    if matches!((query.start_at, query.end_at), (Some(start), Some(end)) if start >= end) {
        return Err(RetrievalError::InvalidTimeRange);
    }
    Ok(())
}

fn decode_visible_entry<E: std::error::Error + 'static>(
    record: RawTimelineRecord,
    grant: RetrievalGrant,
) -> Result<Option<TimelineEntry>, RetrievalError<E>> {
    match record.schema_version {
        1 => {
            let envelope: EventEnvelope =
                serde_json::from_str(&record.envelope_json).map_err(|error| {
                    RetrievalError::MalformedRawEvent {
                        event_id: record.event_id,
                        message: error.to_string(),
                    }
                })?;
            if !sensitivity_allowed(envelope.sensitivity, grant.max_event_sensitivity) {
                return Ok(None);
            }
            let refs = refs_from_v1_payload(&envelope.payload);
            let visible_refs = filter_refs(&refs, grant.max_content_retrieval);
            let payload = if grant.include_payload && visible_refs.len() == refs.len() {
                Some(serde_json::to_value(&envelope.payload).map_err(|error| {
                    RetrievalError::MalformedRawEvent {
                        event_id: record.event_id,
                        message: error.to_string(),
                    }
                })?)
            } else {
                None
            };
            Ok(Some(TimelineEntry {
                event_id: envelope.event_id,
                schema_version: envelope.schema_version,
                event_type: envelope.payload.event_type().into(),
                source: envelope.source,
                source_sequence: envelope.source_sequence,
                occurred_at: None,
                observed_at: envelope.observed_at,
                ingested_at: envelope.ingested_at,
                device_id: None,
                scope_id: envelope.scope_id,
                correlation_id: envelope.correlation_id,
                sensitivity: envelope.sensitivity,
                content_refs: visible_refs,
                payload,
                evidence: envelope.evidence,
            }))
        }
        2 => {
            let envelope: EventEnvelopeV2 =
                serde_json::from_str(&record.envelope_json).map_err(|error| {
                    RetrievalError::MalformedRawEvent {
                        event_id: record.event_id,
                        message: error.to_string(),
                    }
                })?;
            if !sensitivity_allowed(envelope.sensitivity, grant.max_event_sensitivity) {
                return Ok(None);
            }
            let visible_refs = filter_refs(&envelope.content_refs, grant.max_content_retrieval);
            let payload =
                if grant.include_payload && visible_refs.len() == envelope.content_refs.len() {
                    Some(envelope.payload.clone())
                } else {
                    None
                };
            Ok(Some(TimelineEntry {
                event_id: envelope.event_id,
                schema_version: envelope.envelope_version,
                event_type: envelope.event_type,
                source: envelope.source,
                source_sequence: envelope.source_sequence,
                occurred_at: envelope.occurred_at,
                observed_at: envelope.observed_at,
                ingested_at: envelope.ingested_at,
                device_id: envelope.device_id,
                scope_id: envelope.scope_id,
                correlation_id: envelope.correlation_id,
                sensitivity: envelope.sensitivity,
                content_refs: visible_refs,
                payload,
                evidence: envelope.evidence,
            }))
        }
        version => Err(RetrievalError::UnsupportedVersion {
            event_id: record.event_id,
            version,
        }),
    }
}

fn refs_from_v1_payload(payload: &EventPayload) -> Vec<ContentRef> {
    match payload {
        EventPayload::ContentObserved { refs, .. } => refs.clone(),
        _ => Vec::new(),
    }
}

fn filter_refs(refs: &[ContentRef], maximum: RetrievalClass) -> Vec<ContentRef> {
    refs.iter()
        .filter(|reference| {
            retrieval_class_rank(reference.retrieval_class) <= retrieval_class_rank(maximum)
        })
        .cloned()
        .collect()
}

fn sensitivity_allowed(actual: SensitivityClass, maximum: SensitivityClass) -> bool {
    sensitivity_rank(actual) <= sensitivity_rank(maximum)
}

const fn sensitivity_rank(value: SensitivityClass) -> u8 {
    match value {
        SensitivityClass::Metadata => 0,
        SensitivityClass::ContentDerived => 1,
        SensitivityClass::Sensitive => 2,
    }
}

const fn retrieval_class_rank(value: RetrievalClass) -> u8 {
    match value {
        RetrievalClass::Normal => 0,
        RetrievalClass::Sensitive => 1,
        RetrievalClass::Secret => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use context_contracts::{EvidenceDescriptor, EvidenceKind};
    use serde_json::json;

    use super::*;

    struct FakeRepository {
        records: Vec<RawTimelineRecord>,
    }

    impl TimelineRepository for FakeRepository {
        type Error = Infallible;

        fn query_raw_timeline(
            &self,
            query: &RawTimelineQuery,
        ) -> Result<RawTimelinePage, Self::Error> {
            let mut records = self.records.clone();
            records.sort_by(|left, right| {
                right
                    .observed_at
                    .cmp(&left.observed_at)
                    .then_with(|| right.event_id.to_string().cmp(&left.event_id.to_string()))
            });
            records.retain(|record| {
                if let Some(start) = query.start_at
                    && record.observed_at < start
                {
                    return false;
                }
                if let Some(end) = query.end_at
                    && record.observed_at >= end
                {
                    return false;
                }
                if let Some(cursor) = &query.before
                    && !(record.observed_at < cursor.observed_at
                        || (record.observed_at == cursor.observed_at
                            && record.event_id.to_string() < cursor.event_id.to_string()))
                {
                    return false;
                }
                true
            });
            let has_more = records.len() > query.limit;
            records.truncate(query.limit);
            Ok(RawTimelinePage { records, has_more })
        }
    }

    fn evidence() -> EvidenceDescriptor {
        EvidenceDescriptor {
            kind: EvidenceKind::Observed,
            collector: "fixture".into(),
            detail: "fixture".into(),
        }
    }

    fn raw_v2(
        id: Uuid,
        observed_at: OffsetDateTime,
        sensitivity: SensitivityClass,
        refs: Vec<ContentRef>,
        payload: Value,
    ) -> RawTimelineRecord {
        let event = EventEnvelopeV2 {
            event_id: id,
            envelope_version: 2,
            event_type: "fixture.event".into(),
            payload_version: 1,
            source: SourceId("fixture.source".into()),
            source_sequence: None,
            occurred_at: Some(observed_at),
            observed_at,
            ingested_at: observed_at,
            device_id: Some("desktop".into()),
            scope_id: ScopeId("scope.personal".into()),
            correlation_id: None,
            sensitivity,
            content_refs: refs,
            payload,
            evidence: evidence(),
        };
        RawTimelineRecord {
            event_id: id,
            schema_version: 2,
            observed_at,
            envelope_json: serde_json::to_string(&event).unwrap(),
        }
    }

    fn reference(class: RetrievalClass, marker: char) -> ContentRef {
        ContentRef {
            sha256: marker.to_string().repeat(64),
            media_type: "text/plain".into(),
            byte_length: 10,
            compression: None,
            storage_class: "local_vault".into(),
            retrieval_class: class,
        }
    }

    #[test]
    fn metadata_grant_hides_sensitive_events_entirely() {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let public = raw_v2(
            Uuid::now_v7(),
            at,
            SensitivityClass::Metadata,
            Vec::new(),
            json!({"kind": "public"}),
        );
        let sensitive = raw_v2(
            Uuid::now_v7(),
            at + Duration::seconds(1),
            SensitivityClass::Sensitive,
            Vec::new(),
            json!({"url": "https://private.test"}),
        );
        let repository = FakeRepository {
            records: vec![public, sensitive],
        };
        let page = RetrievalEngine::new(&repository)
            .query_timeline(
                &TimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: None,
                    limit: 10,
                },
                RetrievalGrant::metadata_only(),
            )
            .unwrap();

        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].sensitivity, SensitivityClass::Metadata);
        assert!(page.entries[0].payload.is_none());
    }

    #[test]
    fn hidden_content_ref_also_suppresses_payload() {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let event = raw_v2(
            Uuid::now_v7(),
            at,
            SensitivityClass::Sensitive,
            vec![
                reference(RetrievalClass::Sensitive, 'a'),
                reference(RetrievalClass::Secret, 'b'),
            ],
            json!({"content_roles": [{"sha256": "secret-hash"}]}),
        );
        let repository = FakeRepository {
            records: vec![event],
        };
        let page = RetrievalEngine::new(&repository)
            .query_timeline(
                &TimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: None,
                    limit: 10,
                },
                RetrievalGrant {
                    max_event_sensitivity: SensitivityClass::Sensitive,
                    max_content_retrieval: RetrievalClass::Sensitive,
                    include_payload: true,
                },
            )
            .unwrap();

        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].content_refs.len(), 1);
        assert_eq!(
            page.entries[0].content_refs[0].retrieval_class,
            RetrievalClass::Sensitive
        );
        assert!(page.entries[0].payload.is_none());
    }

    #[test]
    fn full_grant_can_return_payload_when_all_refs_are_allowed() {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let event = raw_v2(
            Uuid::now_v7(),
            at,
            SensitivityClass::Sensitive,
            vec![reference(RetrievalClass::Secret, 'c')],
            json!({"answer": 42}),
        );
        let repository = FakeRepository {
            records: vec![event],
        };
        let page = RetrievalEngine::new(&repository)
            .query_timeline(
                &TimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: None,
                    limit: 10,
                },
                RetrievalGrant {
                    max_event_sensitivity: SensitivityClass::Sensitive,
                    max_content_retrieval: RetrievalClass::Secret,
                    include_payload: true,
                },
            )
            .unwrap();

        assert_eq!(page.entries[0].payload.as_ref().unwrap()["answer"], 42);
    }

    #[test]
    fn filtered_rows_advance_the_keyset_cursor() {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let mut records = Vec::new();
        for offset in 0..3 {
            records.push(raw_v2(
                Uuid::now_v7(),
                at + Duration::seconds(offset),
                if offset == 2 {
                    SensitivityClass::Sensitive
                } else {
                    SensitivityClass::Metadata
                },
                Vec::new(),
                json!({"offset": offset}),
            ));
        }
        let repository = FakeRepository { records };
        let first = RetrievalEngine::new(&repository)
            .query_timeline(
                &TimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: None,
                    limit: 1,
                },
                RetrievalGrant::metadata_only(),
            )
            .unwrap();
        assert_eq!(first.entries.len(), 1);
        assert!(first.next_cursor.is_some());

        let second = RetrievalEngine::new(&repository)
            .query_timeline(
                &TimelineQuery {
                    scope_id: ScopeId("scope.personal".into()),
                    start_at: None,
                    end_at: None,
                    before: first.next_cursor,
                    limit: 1,
                },
                RetrievalGrant::metadata_only(),
            )
            .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert_ne!(first.entries[0].event_id, second.entries[0].event_id);
    }
}
