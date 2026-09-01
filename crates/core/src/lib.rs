use context_contracts::{
    CURRENT_SCHEMA_VERSION, EventEnvelope, EventPayload, EvidenceDescriptor, EvidenceKind,
    FileChange, FileIdentity, SensitivityClass, SourceId,
};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectionCommand {
    UpsertTask {
        task_id: Uuid,
        name: String,
        started_at: OffsetDateTime,
    },
    UpsertArtifactLocation {
        identity: FileIdentity,
        path: String,
        scope_id: String,
        observed_at: OffsetDateTime,
        change: FileChange,
        source_event_id: Uuid,
    },
    StorePendingDownload {
        download_id: Uuid,
        url: String,
        referrer: Option<String>,
        final_path: String,
        observed_at: OffsetDateTime,
        source_event_id: Uuid,
    },
    AddObservedDownloadEdge {
        download_id: Uuid,
        identity: FileIdentity,
        url: String,
        source_event_ids: Vec<Uuid>,
        observed_at: OffsetDateTime,
    },
    RecordCollectorGap {
        collector: String,
        last_sequence: Option<u64>,
        reason: String,
        observed_at: OffsetDateTime,
    },
}

pub fn project_event(event: &EventEnvelope) -> Vec<ProjectionCommand> {
    match &event.payload {
        EventPayload::TaskStarted { task_id, name } => vec![ProjectionCommand::UpsertTask {
            task_id: *task_id,
            name: name.clone(),
            started_at: event.observed_at,
        }],
        EventPayload::FileObserved {
            identity,
            path,
            change,
        } => vec![ProjectionCommand::UpsertArtifactLocation {
            identity: identity.clone(),
            path: path.clone(),
            scope_id: event.scope_id.0.clone(),
            observed_at: event.observed_at,
            change: *change,
            source_event_id: event.event_id,
        }],
        EventPayload::BrowserDownloadObserved {
            download_id,
            url,
            referrer,
            final_path,
        } => vec![ProjectionCommand::StorePendingDownload {
            download_id: *download_id,
            url: url.clone(),
            referrer: referrer.clone(),
            final_path: final_path.clone(),
            observed_at: event.observed_at,
            source_event_id: event.event_id,
        }],
        EventPayload::DownloadMatched {
            download_id,
            identity,
            url,
            source_event_ids,
        } => vec![ProjectionCommand::AddObservedDownloadEdge {
            download_id: *download_id,
            identity: identity.clone(),
            url: url.clone(),
            source_event_ids: source_event_ids.clone(),
            observed_at: event.observed_at,
        }],
        EventPayload::CollectorGap {
            collector,
            last_sequence,
            reason,
        } => vec![ProjectionCommand::RecordCollectorGap {
            collector: collector.clone(),
            last_sequence: *last_sequence,
            reason: reason.clone(),
            observed_at: event.observed_at,
        }],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestOutcome {
    Inserted,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DownloadMatchCandidate {
    pub download_id: Uuid,
    pub identity: FileIdentity,
    pub url: String,
    pub source_event_ids: Vec<Uuid>,
    pub observed_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestReport {
    pub outcome: IngestOutcome,
    pub derived_event_ids: Vec<Uuid>,
}

pub trait EventRepository {
    type Error: std::error::Error + Send + Sync + 'static;

    fn append_with_projection(
        &mut self,
        event: &EventEnvelope,
        commands: &[ProjectionCommand],
    ) -> Result<IngestOutcome, Self::Error>;

    fn find_download_match(
        &self,
        path: &str,
        tolerance_seconds: i64,
    ) -> Result<Option<DownloadMatchCandidate>, Self::Error>;
}

#[derive(Debug, Error)]
pub enum IngestError<E: std::error::Error + 'static> {
    #[error("unsupported event schema version {actual}; current version is {expected}")]
    UnsupportedSchema { actual: u16, expected: u16 },
    #[error("repository failed: {0}")]
    Repository(#[source] E),
}

pub struct ContextEngine<R> {
    repository: R,
}

impl<R: EventRepository> ContextEngine<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub fn ingest(&mut self, event: &EventEnvelope) -> Result<IngestReport, IngestError<R::Error>> {
        if event.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(IngestError::UnsupportedSchema {
                actual: event.schema_version,
                expected: CURRENT_SCHEMA_VERSION,
            });
        }

        let commands = project_event(event);
        let outcome = self
            .repository
            .append_with_projection(event, &commands)
            .map_err(IngestError::Repository)?;
        let path = match &event.payload {
            EventPayload::FileObserved { path, change, .. }
                if !matches!(change, FileChange::Deleted) =>
            {
                Some(path.as_str())
            }
            EventPayload::BrowserDownloadObserved { final_path, .. } => Some(final_path.as_str()),
            _ => None,
        };
        let Some(path) = path else {
            return Ok(IngestReport {
                outcome,
                derived_event_ids: Vec::new(),
            });
        };
        let Some(candidate) = self
            .repository
            .find_download_match(path, 180)
            .map_err(IngestError::Repository)?
        else {
            return Ok(IngestReport {
                outcome,
                derived_event_ids: Vec::new(),
            });
        };

        let derived = EventEnvelope {
            event_id: Uuid::now_v7(),
            schema_version: CURRENT_SCHEMA_VERSION,
            source: SourceId("core.download-correlator".into()),
            source_sequence: None,
            observed_at: candidate.observed_at,
            ingested_at: OffsetDateTime::now_utc(),
            scope_id: event.scope_id.clone(),
            correlation_id: Some(event.event_id),
            sensitivity: SensitivityClass::Metadata,
            payload: EventPayload::DownloadMatched {
                download_id: candidate.download_id,
                identity: candidate.identity,
                url: candidate.url,
                source_event_ids: candidate.source_event_ids,
            },
            evidence: EvidenceDescriptor {
                kind: EvidenceKind::Observed,
                collector: "download-correlator-v1".into(),
                detail: "exact final path with observed events within 180 seconds".into(),
            },
        };
        let derived_commands = project_event(&derived);
        self.repository
            .append_with_projection(&derived, &derived_commands)
            .map_err(IngestError::Repository)?;

        Ok(IngestReport {
            outcome,
            derived_event_ids: vec![derived.event_id],
        })
    }

    pub fn repository(&self) -> &R {
        &self.repository
    }

    pub fn repository_mut(&mut self) -> &mut R {
        &mut self.repository
    }

    pub fn into_repository(self) -> R {
        self.repository
    }
}
