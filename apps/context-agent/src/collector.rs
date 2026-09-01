use std::{collections::HashSet, path::PathBuf};

use anyhow::{Context, Result, bail};
use context_contracts::{EventEnvelope, EventPayload, FileChange, FileIdentity};
use context_core::ContextEngine;
use context_platform_windows::{
    DirectoryAction, DirectoryBatch, DirectoryChange, DirectoryWatcher, ReconcileIssue,
    WatchCancellation, WatchOutcome, contract_path, file_identity, scan_scope,
};
use context_storage_sqlite::{ActiveLocation, SqliteRepository};
use time::OffsetDateTime;

const SOURCE: &str = "windows.directory-watcher";

pub fn run(root: PathBuf, database_path: PathBuf, max_batches: Option<usize>) -> Result<()> {
    let mut state = CollectorState::open(root, database_path)?;

    let startup = state.reconcile()?;
    println!(
        "collector startup reconciliation: observed={}; inserted={}; deleted={}; issues={}",
        startup.observed, startup.inserted, startup.deleted, startup.issues
    );

    let cancellation = WatchCancellation::new()?;
    if max_batches.is_none() {
        let signal = cancellation.clone();
        ctrlc::set_handler(move || {
            let _ = signal.cancel();
        })
        .context("install collector cancellation handler")?;
    }
    let mut watcher = DirectoryWatcher::open(&state.root, true, 64 * 1024)
        .with_context(|| format!("watch directory {}", state.root.display()))?;
    let mut completed_batches = 0usize;
    loop {
        match watcher.read_next(&cancellation)? {
            WatchOutcome::Cancelled => break,
            WatchOutcome::Batch(batch) => {
                let gap = batch.gap_detected;
                let changes = batch.changes.len();
                let ingested = state.ingest_batch(batch)?;
                completed_batches += 1;
                println!(
                    "collector batch: number={completed_batches}; changes={changes}; ingested={ingested}; gap={gap}; sequence={}",
                    state.last_sequence
                );
                if gap {
                    let recovery = state.reconcile()?;
                    println!(
                        "collector recovery reconciliation: observed={}; inserted={}; deleted={}; issues={}",
                        recovery.observed, recovery.inserted, recovery.deleted, recovery.issues
                    );
                }
                if max_batches.is_some_and(|maximum| completed_batches >= maximum) {
                    break;
                }
            }
        }
    }
    println!(
        "collector stopped: batches={completed_batches}; last_sequence={}",
        state.last_sequence
    );
    Ok(())
}

pub(crate) struct CollectorState {
    engine: ContextEngine<SqliteRepository>,
    root: PathBuf,
    scope_id: String,
    last_sequence: u64,
}

impl CollectorState {
    pub(crate) fn open(root: PathBuf, database_path: PathBuf) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("resolve scope root {}", root.display()))?;
        reject_database_inside_scope(&root, &database_path)?;
        let repository = SqliteRepository::open(&database_path)
            .with_context(|| format!("open database {}", database_path.display()))?;
        let scope_id = contract_path(&root);
        let last_sequence = repository
            .last_source_sequence(SOURCE, &scope_id)?
            .unwrap_or(0);
        Ok(Self {
            engine: ContextEngine::new(repository),
            root,
            scope_id,
            last_sequence,
        })
    }

    pub(crate) fn root(&self) -> &PathBuf {
        &self.root
    }

    pub(crate) fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub(crate) fn engine_mut(&mut self) -> &mut ContextEngine<SqliteRepository> {
        &mut self.engine
    }

    pub(crate) fn ingest_batch(&mut self, batch: DirectoryBatch) -> Result<usize> {
        let mut ingested = 0usize;
        if batch.gap_detected {
            self.record_gap(
                "ReadDirectoryChangesExW returned zero bytes; scope reconciliation required",
            )?;
            ingested += 1;
        }
        let mut unresolved_identity = false;
        for change in batch.changes {
            match self.ingest_change(change)? {
                ChangeOutcome::Ingested => ingested += 1,
                ChangeOutcome::Ignored => {}
                ChangeOutcome::NeedsReconciliation => unresolved_identity = true,
            }
        }
        if unresolved_identity {
            self.record_gap("a directory change had no resolvable stable file identity")?;
            ingested += 1;
            let _ = self.reconcile()?;
        }
        Ok(ingested)
    }

    fn ingest_change(&mut self, change: DirectoryChange) -> Result<ChangeOutcome> {
        let file_change = match change.action {
            DirectoryAction::Added => FileChange::Created,
            DirectoryAction::Modified => FileChange::Modified,
            DirectoryAction::RenamedTo => FileChange::Renamed,
            DirectoryAction::Removed | DirectoryAction::RenamedFrom => FileChange::Deleted,
            DirectoryAction::Unknown(_) => return Ok(ChangeOutcome::Ignored),
        };
        let path = self.root.join(change.relative_path);
        let identity = match change.identity {
            Some(identity) => identity,
            None if !matches!(file_change, FileChange::Deleted) => match file_identity(&path) {
                Ok(identity) => identity,
                Err(_) => return Ok(ChangeOutcome::NeedsReconciliation),
            },
            None => return Ok(ChangeOutcome::NeedsReconciliation),
        };
        self.ingest_file(
            identity,
            path,
            file_change,
            "ReadDirectoryChangesExW extended record",
        )?;
        Ok(ChangeOutcome::Ingested)
    }

    pub(crate) fn reconcile(&mut self) -> Result<ReconcileSummary> {
        let report = scan_scope(&self.root)?;
        let active = self
            .engine
            .repository()
            .active_locations_in_scope(&self.scope_id)?;
        let current: HashSet<(FileIdentity, String)> = report
            .files
            .iter()
            .map(|file| (file.identity.clone(), contract_path(&file.path)))
            .collect();
        let existing: HashSet<(FileIdentity, String)> = active
            .iter()
            .map(|location| (location.identity.clone(), location.path.clone()))
            .collect();
        let mut inserted = 0usize;
        for file in &report.files {
            let path = contract_path(&file.path);
            if !existing.contains(&(file.identity.clone(), path)) {
                self.ingest_file(
                    file.identity.clone(),
                    file.path.clone(),
                    FileChange::Modified,
                    "scope reconciliation observed current file",
                )?;
                inserted += 1;
            }
        }

        let mut deleted = 0usize;
        for location in active {
            if current.contains(&(location.identity.clone(), location.path.clone()))
                || path_is_under_issue(&location, &report.issues)
            {
                continue;
            }
            self.ingest_file(
                location.identity,
                PathBuf::from(location.path),
                FileChange::Deleted,
                "scope reconciliation did not find prior active location",
            )?;
            deleted += 1;
        }

        for issue in &report.issues {
            self.record_gap(&format!(
                "scope reconciliation could not inspect {}: {}",
                issue.path.display(),
                issue.message
            ))?;
        }
        if report.issues.is_empty() {
            self.engine.repository_mut().mark_collector_reconciled(
                SOURCE,
                &self.scope_id,
                self.last_sequence,
            )?;
        }
        Ok(ReconcileSummary {
            observed: report.files.len(),
            inserted,
            deleted,
            issues: report.issues.len(),
        })
    }

    fn ingest_file(
        &mut self,
        identity: FileIdentity,
        path: PathBuf,
        change: FileChange,
        detail: &str,
    ) -> Result<()> {
        let payload = EventPayload::FileObserved {
            identity,
            path: contract_path(path),
            change,
        };
        self.ingest(payload, detail)
    }

    fn record_gap(&mut self, reason: &str) -> Result<()> {
        let previous = self.last_sequence;
        self.ingest(
            EventPayload::CollectorGap {
                collector: SOURCE.into(),
                last_sequence: Some(previous),
                reason: reason.into(),
            },
            "collector gap requires bounded scope reconciliation",
        )
    }

    fn ingest(&mut self, payload: EventPayload, detail: &str) -> Result<()> {
        self.last_sequence = self
            .last_sequence
            .checked_add(1)
            .context("collector source sequence exhausted")?;
        let mut event = EventEnvelope::observed(
            SOURCE,
            self.scope_id.clone(),
            OffsetDateTime::now_utc(),
            payload,
            "context-agent",
            detail,
        );
        event.source_sequence = Some(self.last_sequence);
        self.engine.ingest(&event)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangeOutcome {
    Ingested,
    Ignored,
    NeedsReconciliation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReconcileSummary {
    pub(crate) observed: usize,
    pub(crate) inserted: usize,
    pub(crate) deleted: usize,
    pub(crate) issues: usize,
}

fn path_is_under_issue(location: &ActiveLocation, issues: &[ReconcileIssue]) -> bool {
    let path = PathBuf::from(&location.path);
    issues
        .iter()
        .any(|issue| path.starts_with(contract_path(&issue.path)))
}

fn reject_database_inside_scope(root: &PathBuf, database_path: &PathBuf) -> Result<()> {
    let absolute_database = if database_path.is_absolute() {
        database_path.clone()
    } else {
        std::env::current_dir()?.join(database_path)
    };
    let database_parent = absolute_database
        .parent()
        .context("database path has no parent directory")?;
    let resolved_parent = database_parent
        .canonicalize()
        .with_context(|| format!("resolve database parent {}", database_parent.display()))?;
    if resolved_parent.starts_with(root) {
        bail!("database must be outside the watched scope to prevent self-generated event loops");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use context_platform_windows::ReconcileIssueKind;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn unreadable_subtree_protects_prior_locations_from_false_deletion() {
        let location = ActiveLocation {
            identity: FileIdentity {
                provider: "fixture".into(),
                namespace: "volume".into(),
                opaque_id: vec![1],
            },
            path: PathBuf::from("scope")
                .join("denied")
                .join("kept.txt")
                .to_string_lossy()
                .into_owned(),
        };
        let issues = vec![ReconcileIssue {
            path: PathBuf::from("scope").join("denied"),
            kind: ReconcileIssueKind::AccessDenied,
            message: "access denied".into(),
        }];

        assert!(path_is_under_issue(&location, &issues));
    }

    #[test]
    fn database_inside_scope_is_rejected_before_collection() {
        let root = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let resolved_root = root.canonicalize().unwrap();

        let error = reject_database_inside_scope(&resolved_root, &data.join("context.db"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("outside the watched scope"));
        fs::remove_dir_all(root).unwrap();
    }
}
