# Context Layer

Local-first task context and provenance layer for desktop operating systems. Windows-first.

This repository is an architecture-first alpha scaffold. It currently proves the contracts that future collectors, storage, task views, and platform adapters must share. It is not yet an end-user application.

## Current vertical slice

- versioned, typed event envelope;
- deterministic projection commands;
- atomic SQLite event + projection writes;
- duplicate-event idempotency;
- stable Windows file identity across rename;
- automatic download/file correlation in either arrival order;
- duplicate replay repair after an interrupted derived projection;
- forward-only, version-checked SQLite schema migration;
- architecture decisions and threat model.

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

## Non-goals for the first foundation release

No cloud account, sync, Windows service, kernel driver, screen capture, automatic file moves, automatic task switching, plugin marketplace, or administrator-only baseline.

Licensed under Apache-2.0. See `LICENSE`.
