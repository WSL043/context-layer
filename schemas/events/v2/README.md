# Event schema v2

This directory contains compatibility fixtures for the open Personal Context v2 event envelope.

The v2 contract separates a stable envelope from collector-specific payload evolution:

- `envelope_version` versions the transport/storage contract;
- `event_type` names the observation independently of Rust enums;
- `payload_version` belongs to that event type;
- `payload` remains opaque JSON until a projector explicitly understands that type/version;
- `occurred_at`, `observed_at`, and `ingested_at` keep source time, observation time, and durable-ingest time distinct;
- content bytes stay outside the envelope and are referenced through `content_refs`.

A v2-capable core rejects an unsupported envelope version because it cannot safely promise preservation of an envelope shape it does not understand. Within envelope v2, however, unknown event types and newer payload versions are valid raw evidence and must be retained even when they produce no semantic projection.

`unknown_payload.json` is the contract fixture for that guarantee.
