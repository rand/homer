# homer-test

Integration tests and fixture utilities for Homer.

## Tests

`tests/pipeline.rs` contains 14 integration tests covering:

- End-to-end pipeline execution (extract → analyze → render)
- Auto snapshot creation (release-triggered and commit-count-triggered)
- Successive commit-count snapshots
- Empty directory handling
- Snapshot idempotency
- Pipeline fault tolerance (git errors don't abort other extractors)

## Test Utilities

Tests use:
- `tempfile::tempdir()` for isolated test directories
- `SqliteStore::in_memory()` for fast, ephemeral databases
- `HomerConfig::default()` with targeted overrides

## Running

```bash
cargo test -p homer-test
```

Or as part of the full workspace:

```bash
cargo test --workspace
```
