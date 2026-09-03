# Foundation threat model

## Assets

File locations, URLs, task names, inferred relationships, exclusion rules, raw events, local raw-content blobs, local encryption keys, update signing keys, and browser Native Messaging registration.

## Trust boundaries

- browser extension to Native Messaging host;
- Native Messaging host to current-user agent;
- platform collector to typed event contract;
- capture backend to Context Layer adapter;
- UI to local command/query API;
- agent to SQLite, local content vault, and OS key store;
- future agent/retrieval clients to the Context Layer query boundary;
- release workflow to signed installer and updater metadata.

## Initial controls

- no network service, account, or cloud storage;
- current-user process and IPC ACL, no Windows service;
- Named Pipe rejects remote clients and its protected DACL grants access only to LocalSystem and the current user SID;
- explicit path/app/domain scopes before collection in the foundation profile;
- length-delimited, size-limited, typed IPC messages;
- Native Messaging requires an exact allowlisted extension origin and validates URL scheme/host and absolute Windows paths;
- browser delivery retries use stable event UUIDs; a 256-message outbox limit produces an explicit collector gap instead of silent loss;
- reconciliation never treats an unreadable subtree as deleted and does not follow Windows reparse points;
- append-only evidence with detector version and status;
- UI has no database access;
- update artifacts are signed and checksummed;
- deletion tests cover raw events, projections, caches, vault references, and backups.

## Personal capture profile

Personal Context v2 intentionally separates **capture policy** from **retrieval policy**.

A user may opt into broad local capture, including private conversations, browsing, screenshots, clipboard content, and other personal activity. Broad capture does not imply that every future agent or retrieval client may read all retained evidence.

Content references carry a retrieval class:

- `normal`: ordinary activity and content;
- `sensitive`: private conversations, documents, and personal information;
- `secret`: credentials, recovery codes, private keys, authentication material, and similarly dangerous content.

The normal query path should apply least privilege and require explicit authorization before exposing `sensitive` or `secret` evidence. Secret material may be retained locally when the user chooses broad capture, but it must not be silently inserted into ordinary model context, summaries, exports, logs, or telemetry.

Raw blob bytes belong in the local content vault rather than `raw_event.envelope_json`. Event records retain hashes and metadata so deletion, provenance, deduplication, and retrieval authorization can be enforced without making SQLite itself the raw-content store.

## Logging rule

Operational logs remain different from captured evidence. Logs must not echo raw document/chat/page contents, credentials, or full secret-bearing payloads merely because the local evidence store is allowed to retain them.

## Explicitly out of scope

The foundation does not claim to resist malware already executing as the same Windows user. DPAPI protects against offline access, not a fully compromised interactive session.

Personal Context v2 also does not claim that local storage alone makes arbitrary agent access safe. Retrieval authorization remains a separate boundary even when all data stays on the same machine.
