# Personal Context v2

This document defines the next foundation layer for Context Layer: a local-first personal context backbone that can record digital activity broadly while keeping raw content local and provenance-preserving.

## Product boundary

Context Layer is not a screen recorder and not a task tracker. It is the durable context backbone that links what happened, when it happened, where the evidence lives, and what later semantic systems infer from that evidence.

Collectors are replaceable. Raw observations are durable. Semantic interpretation is rebuildable.

## Design goals

1. Capture broad personal/work activity without requiring manual task switching.
2. Preserve raw observations even when the current binary does not understand a newer payload.
3. Keep large or sensitive content outside SQLite in a content-addressed local vault.
4. Separate observed facts from inferred semantics and user-confirmed facts.
5. Allow future models to reprocess old evidence without mutating or deleting the original record.
6. Separate capture policy from retrieval policy: local capture can be broad while agents receive least-privilege views.
7. Support multi-device and delayed-observation data by distinguishing occurrence, observation, and ingestion time.

## Layer model

```text
Collectors
  -> versioned event envelope
  -> append-only raw event log
  -> content-addressed raw vault
  -> deterministic projections
  -> semantic projections
  -> personal graph / open loops / routines / decisions
  -> retrieval API
  -> secretary and work agents
```

## Event compatibility

The envelope and payload evolve independently.

- `envelope_version` describes the stable transport/envelope contract.
- `event_type` is a namespaced identifier such as `browser.page_observed` or `ui.snapshot_observed`.
- `payload_version` versions only that event payload.
- Unknown event types or newer payload versions must remain storable as raw evidence even if no projection understands them yet.
- Projection code must opt into event types/versions it understands; unsupported events are retained, not rejected.
- A binary must reject an envelope version it does not understand; forward retention is guaranteed inside the v2 envelope, not across unknown future envelope shapes.

The existing strongly typed Rust payloads remain useful for built-in collectors. They should be treated as typed views over the durable raw envelope rather than the only form that can cross the ingest boundary forever.

The first v2 implementation deliberately keeps complete v2 envelopes in the canonical `raw_event` timeline. It does not create ad-hoc indexing tables outside the normal database migration system. Event-type/time indexes and semantic projections should be introduced later through explicit versioned migrations when query workloads exist to justify them.

## Time model

Personal context needs three clocks:

- `occurred_at`: when the source says the underlying event happened, when known.
- `observed_at`: when the collector observed it.
- `ingested_at`: when Context Layer durably accepted it.

For live local events these may be nearly identical. Imported chat history, notifications, delayed browser delivery, sleep/resume and cross-device imports make the distinction necessary.

`ingested_at` is stamped at the trusted Context Layer ingest boundary. Collectors may report occurrence and observation time, but they cannot authoritatively choose the durable-ingest time stored as evidence.

## Raw content vault

Large content is referenced, not embedded in `raw_event.envelope_json`.

A `ContentRef` identifies a local blob using a content hash and records only metadata needed to retrieve and verify it:

```text
content_id / sha256
media_type
byte_length
compression
storage_class
retrieval_class
```

Recommended local layout:

```text
data/
  context.db
  vault/
    blobs/
      aa/bb/<sha256>
  index/
  export/
```

The vault should use atomic writes, hash verification, deduplication, and immutable blob contents. Lifecycle/deletion policy belongs above the blob store; a blob can be deleted only when no retained event references it.

## Capture vs retrieval policy

Personal mode intentionally separates two questions:

- What is retained locally?
- What is an agent allowed to retrieve?

Suggested retrieval classes:

- `normal`: ordinary activity and content.
- `sensitive`: private conversations, documents, personal information.
- `secret`: credentials, recovery codes, private keys and similarly dangerous material.

A user may choose broad local capture while ordinary agents still receive only `normal` or explicitly authorized `sensitive` evidence. Capture breadth must not imply universal agent access.

## Observation and interpretation

Raw observations are immutable evidence. Derived understanding is versioned and replaceable.

Examples:

```text
ui snapshot + OCR
  -> conversation message
  -> commitment
  -> open loop
```

or

```text
foreground window + active tab + dwell + scroll
  -> reading episode
  -> research task
  -> project relationship
```

Every inferred claim must keep:

- extractor/detector identity and version;
- creation time;
- confidence when applicable;
- source event IDs / content refs;
- status (`inferred`, `confirmed`, `rejected`, `superseded`).

Future models should be able to rebuild semantic projections from the same raw evidence.

## Activity hierarchy

Do not require the user to manually start a task for ordinary use.

```text
Event
  -> Activity Block
  -> Episode
  -> Task
  -> Project
```

Explicit user tasks remain valuable high-confidence evidence, but automatic sessionization is required for personal mode.

## First semantic entities

The first useful personal graph should be small and evidence-backed:

- Person
- Organization
- Project
- Product
- Conversation
- Document / Artifact
- Website / Resource
- Topic
- Decision
- Preference
- Routine
- Commitment
- OpenLoop

`OpenLoop` is deliberately first-class: many obligations and awaited responses never become explicit todos.

## Collector roadmap

Foundation order:

1. foreground window/process + idle state;
2. clipboard metadata/content reference;
3. screen/UI backend adapter (Screenpipe can be the first replaceable backend);
4. browser active-tab/navigation/dwell events;
5. browser visible text / selection / copy provenance;
6. chat semantic extraction (WeChat first on Windows where feasible);
7. development hooks (Git, terminal, editor, agent runs);
8. optional high-frequency demonstration mode for agent training trajectories.

## 30-day acceptance target

Before building a secretary UI, Context Layer should run continuously for 30 days and make it possible to reconstruct most of an arbitrary day's digital activity with evidence.

A successful foundation should answer questions such as:

- What was I doing between 14:00 and 17:00 yesterday?
- What pages did I actually read rather than merely leave open?
- What did I discuss with a specific person and what remains unresolved?
- What information did I copy from one source into another workflow?
- Which activities recur often enough to be automation candidates?

The first milestone is durable observation and reconstruction, not autonomous action.
