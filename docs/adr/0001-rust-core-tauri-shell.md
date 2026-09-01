# ADR 0001: Rust core with a replaceable Tauri shell

Status: Accepted for the Windows alpha foundation.

## Context

The product must start on Windows while preserving domain contracts for later macOS and Linux adapters. A long-running collector should have low idle overhead, while the task UI will change more frequently than the identity and provenance model.

## Decision

Use Rust for contracts, domain capabilities, storage ports, the agent runtime, and native adapters. Use Tauri 2 with a TypeScript frontend for the desktop shell. The shell communicates through a versioned local API and never opens the database.

## Consequences

- Platform adapters and UI can be replaced independently.
- Rust/TypeScript IPC must be contract-tested.
- Tauri is a delivery choice, not a dependency of the domain core.
- We do not add a second language to the agent until a concrete platform API requires it.
