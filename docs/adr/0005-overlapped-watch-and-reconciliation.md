# ADR 0005: Overlapped watch with transactional checkpoints and bounded reconciliation

Status: Accepted

## Context

Directory change notifications are not a durable log. Buffers can overflow, the
agent can be offline, files can disappear before a second path lookup, and one
subtree can deny access while the rest of the selected scope remains readable.
A watcher-only implementation would silently diverge from disk state.

## Decision

`platform-windows` owns an overlapped `ReadDirectoryChangesExW` adapter. Extended
records carry the Windows File ID even for rename/removal events. A manual-reset
event makes an outstanding read cancellable without polling.

`context-agent` converts batches to versioned events and owns reconciliation
policy. It scans once at startup and after every explicit gap, does not follow
reparse points, continues past unreadable subtrees, and never marks paths below an
unreadable subtree as deleted.

SQLite schema v2 stores `(source, scope, last_sequence, reconciliation_required)`.
The source sequence and raw event update that checkpoint in the same transaction.
A clean scan clears the flag; any isolated scan issue records another gap.

## Consequences

- Restart and buffer overflow converge through the same tested path.
- Database files are rejected inside watched scopes to prevent feedback loops.
- Reconciliation may be expensive for very large roots; later work may add
  budgets and persisted directory fingerprints without changing event contracts.
- Platform adapters on macOS/Linux must provide equivalent gap and cancellation
  semantics, but do not copy Windows API details.
