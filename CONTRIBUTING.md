# Contributing

The project is architecture-first. A change is ready only when it preserves evidence semantics, least privilege, and rebuildable projections.

Before submitting a change:

1. Add or update an ADR when changing an irreversible boundary.
2. Add a serialized contract fixture when changing event fields.
3. Keep platform adapters from writing storage or constructing provenance edges.
4. Include a failure/recovery test for collectors and derived projections.
5. Run formatting, Clippy with warnings denied, and all workspace tests.

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Do not include real browsing history, file paths, document content, secrets, or signing keys in tests or issues.
