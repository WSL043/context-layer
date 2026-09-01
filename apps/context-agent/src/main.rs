use std::{env, path::PathBuf};

use anyhow::{Context, Result, bail};
use context_contracts::{EventEnvelope, EventPayload};
use context_core::ContextEngine;
use context_storage_sqlite::SqliteRepository;
use time::OffsetDateTime;
use uuid::Uuid;

fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        println!("context-agent foundation scaffold");
        println!("usage: context-agent --self-check [database-path]");
        return Ok(());
    };
    if command != "--self-check" {
        bail!("unknown command: {}", command.to_string_lossy());
    }

    let database_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("context.db"));
    let repository = SqliteRepository::open(&database_path)
        .with_context(|| format!("open database {}", database_path.display()))?;
    let mut engine = ContextEngine::new(repository);
    let event = EventEnvelope::observed(
        "agent.self-check",
        "scope.self-check",
        OffsetDateTime::now_utc(),
        EventPayload::TaskStarted {
            task_id: Uuid::now_v7(),
            name: "Agent self-check".into(),
        },
        "context-agent",
        "local self-check",
    );
    let report = engine.ingest(&event)?;

    println!(
        "self-check: {:?}; raw_events={}; database={}",
        report.outcome,
        engine.repository().raw_event_count()?,
        database_path.display()
    );
    Ok(())
}
