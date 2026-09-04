# Context Layer

Local-first task context and provenance layer for desktop operating systems. Windows-first.

“Windows-first” defines delivery order, not a Windows-only product and not
cross-device sync. The contracts, core, storage rules, and client API are shared;
macOS and Linux later supply their own collectors, local transport, and packages.

This repository is an architecture-first alpha scaffold. It currently proves the contracts that future collectors, storage, task views, and platform adapters must share. It is not yet an end-user application.

## Current vertical slice

- legacy typed v1 events plus an open v2 raw envelope for unknown/future collector payloads;
- distinct occurrence, observation, and authoritative ingestion clocks;
- deterministic projection commands for built-in typed events;
- atomic SQLite event + projection writes;
- duplicate-event idempotency and immutable raw evidence;
- stable Windows file identity across rename;
- cancellable overlapped `ReadDirectoryChangesExW` batches with File IDs and explicit gap semantics;
- persistent per-scope source checkpoints and startup/gap reconciliation;
- sparse Windows foreground-window/process/title and input-idle activity capture;
- bounded Windows Unicode clipboard capture whose body is stored only in the content-addressed vault and referenced from sensitive v2 events;
- an optional Screenpipe localhost REST adapter that retains discovered screen text and frame PNGs in the same vault without reading Screenpipe's internal database;
- a 1 MiB-capped, versioned local JSON framing contract;
- a local-only Named Pipe protected by a current-user SID DACL;
- a Native Messaging host that validates origin, URLs, paths, bridge protocol, and Local API protocol independently;
- a Manifest V3 Chromium extension with a bounded durable delivery outbox;
- active HTTP(S) page state capture on tab/page/browser-focus transitions, including full URL and title as sensitive evidence;
- automatic download/file correlation in either arrival order;
- duplicate replay repair after an interrupted derived projection;
- forward-only, version-checked SQLite schema migration;
- content references with separate retrieval classes for normal, sensitive, and secret evidence;
- a content-addressed local raw vault that stores blobs outside SQLite and deduplicates by SHA-256;
- architecture decisions and threat model.

## Personal Context v2 direction

The next layer expands Context Layer from file/task provenance into a durable local personal-context backbone. Screen/UI capture, browser activity, chat extraction, clipboard history, development tools, and future phone/import collectors are treated as replaceable evidence sources rather than separate memory products.

Raw observations remain durable; semantic interpretation remains rebuildable. Large content stays in the local raw vault and events carry references instead of embedding screenshot/audio/page bytes in SQLite. Capture policy and agent retrieval policy are intentionally separate so broad local retention does not grant every future agent unrestricted access.

The staged architecture and 30-day acceptance target are documented in [`docs/personal-context-v2.md`](docs/personal-context-v2.md).

## Dependency direction

`platform adapters -> contracts -> core <- storage`, composed only by `context-agent`.

Platform adapters emit events. They cannot write SQLite or create provenance edges. The UI will communicate with the agent through a versioned local API and will never access database tables directly.

## Build and test

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Run the current agent self-check against a local database:

```powershell
cargo run -p context-agent -- --self-check .\context.db
```

Observe one real Windows directory change and persist it:

```powershell
cargo run -p context-agent -- --watch-once C:\path\you\selected .\context.db
```

Run the unified Agent until Ctrl+C, keeping its database outside the selected
scope. This is the normal development runtime: one process owns SQLite while
serving Native Host requests, the Windows file watcher, sparse foreground /
input-idle sampling, and bounded clipboard observation. Raw clipboard and screen
content is written to `vault/blobs` beside the database; SQLite stores event
metadata and content references rather than the raw bodies:

```powershell
cargo run -p context-agent -- --run C:\path\you\selected .\data\context.db
```

With the unpacked Chromium extension and allowlisted Native Host installed, the
same Agent also receives durable browser download and active-page events. Active
page capture records HTTP(S) URL/title state changes and browser focus boundaries;
it does not read page DOM/content in this slice.

### Optional Screenpipe adapter

The Screenpipe adapter is disabled unless a local API key is present. It uses only
the documented localhost REST API; it does not read Screenpipe's SQLite tables or
other internal storage.

```powershell
$env:SCREENPIPE_LOCAL_API_URL = "http://localhost:3030" # optional; this is the default
$env:SCREENPIPE_LOCAL_API_KEY = "<your local Screenpipe API key>"
cargo run -p context-agent -- --run C:\path\you\selected .\data\context.db
```

For compatibility, `SCREENPIPE_API_KEY` is accepted when the newer local-key
environment variable is absent. The adapter rejects HTTPS, credentials, remote
hosts, URL paths, queries, and fragments; only plain HTTP on localhost/loopback is
accepted.

On first enablement it imports only the latest five minutes of screen-text frames
discoverable through Screenpipe's official Search API. After a frame is durably
stored, its Screenpipe `frame_id` and capture timestamp are part of the canonical
raw event; restart recovery uses that event as the cursor, with a small overlap for
safe replay. Accessibility text is preferred when available and OCR is the
fallback for the same frame. The frame PNG and retained text are copied into the
Context Layer vault, while the v2 event stores hashes, capture metadata, and
explicit missing/oversized statuses.

For bounded diagnostics, `--run-batches <directory> <count> [database]` exits
after the requested number of watcher batches. The collector-only equivalents
remain available for isolating watcher/reconciliation faults. `--collector-health`
reports persisted sequence/reconciliation state plus event, location, and
download-correlation counts.

The one-shot pipe server and Native Host agent diagnostic exercise the process
boundary used by the browser bridge:

```powershell
# terminal 1
cargo run -p context-agent -- --serve-once .\context.db

# terminal 2
cargo run -p context-native-host -- --agent-self-check
```

## Non-goals for the first foundation release

No cloud account, sync, Windows service, kernel driver, automatic file moves, automatic task switching, plugin marketplace, or administrator-only baseline.

Screen/UI evidence stays behind a replaceable backend boundary. Screenpipe is the first adapter, not a storage or core dependency: disabling or replacing it must not change the event/vault contract.

`apps/browser-extension` is an unpacked contract alpha. It is not registered in
the browser by build or test commands; registration belongs to the per-user
installer after a real extension ID is selected.

Licensed under Apache-2.0. See `LICENSE`.
