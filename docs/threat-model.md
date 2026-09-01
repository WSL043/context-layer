# Foundation threat model

## Assets

File locations, URLs, task names, inferred relationships, exclusion rules, raw events, local encryption keys, update signing keys, and browser Native Messaging registration.

## Trust boundaries

- browser extension to Native Messaging host;
- Native Messaging host to current-user agent;
- platform collector to typed event contract;
- UI to local command/query API;
- agent to SQLite and OS key store;
- release workflow to signed installer and updater metadata.

## Initial controls

- no network service, account, or cloud storage;
- current-user process and IPC ACL, no Windows service;
- explicit path/app/domain scopes before collection;
- length-delimited, size-limited, typed IPC messages;
- append-only evidence with detector version and status;
- UI has no database access;
- sensitive logs omit document content and URLs by default;
- update artifacts are signed and checksummed;
- deletion tests cover raw events, projections, caches, and backups.

## Explicitly out of scope

The foundation does not claim to resist malware already executing as the same Windows user. DPAPI protects against offline access, not a fully compromised interactive session.
