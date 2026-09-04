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
input-idle sampling, and bounded clipboard observation. Clipboard text bytes are
written to `vault/blobs` beside the database; SQLite stores only the event metadata
and content reference:

```powershell
cargo run -p context-agent -- --run C:\path\you\selected .\data\context.db
```

With the unpacked Chromium extension and allowlisted Native Host installed, the
same Agent also receives durable browser download and active-page events. Active
page capture records HTTP(S) URL/title state changes and browser focus boundaries;
it does not read page DOM/content in this slice.

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

No cloud account, sync, Windows service, kernel driver, screen capture, automatic file moves, automatic task switching, plugin marketplace, or administrator-only baseline.

Screen capture remains outside the first foundation release itself; Personal Context v2 will integrate it behind a replaceable collector/backend boundary rather than coupling the core to one capture implementation.

`apps/browser-extension` is an unpacked contract alpha. It is not registered in
the browser by build or test commands; registration belongs to the per-user
installer after a real extension ID is selected.

Licensed under Apache-2.0. See `LICENSE`.
