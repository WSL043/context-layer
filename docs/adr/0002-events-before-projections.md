# ADR 0002: Append events before rebuilding projections

Status: Accepted.

## Context

Identity and correlation algorithms will change as real evidence is collected. Storing only the latest graph would make algorithm upgrades irreversible and would blur observed evidence with inferred relationships.

## Decision

Persist each validated event idempotently, then apply projection commands in the same SQLite transaction. Raw events are immutable. Identity, location, provenance, task, and ranking state are versioned projections that can be rebuilt.

## Consequences

- Duplicate delivery is safe.
- Detector changes do not rewrite evidence.
- Retention and deletion must cover raw events and every projection.
- Schema compatibility is tested with serialized fixtures before releases.
