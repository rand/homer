# Performance Regression Checks

Homer maintains explicit performance gates for three critical paths:
- parse/extract path (structure + graph extraction)
- centrality analysis path
- incremental no-op update path

The checks are implemented as ignored Rust integration tests in
`homer-core/tests/perf_regression.rs`.

## Run Locally

```bash
scripts/perf-regression.sh
```

Equivalent command:

```bash
cargo test -p homer-core --test perf_regression -- --ignored --nocapture
```

## Threshold Overrides

Thresholds are configured via environment variables (milliseconds):
- `HOMER_PERF_STRUCTURE_MS` (default: `5000`)
- `HOMER_PERF_PARSE_MS` (default: `10000`)
- `HOMER_PERF_CENTRALITY_MS` (default: `10000`)
- `HOMER_PERF_INCREMENTAL_MS` (default: `6000`)

Example:

```bash
HOMER_PERF_INCREMENTAL_MS=4000 scripts/perf-regression.sh
```

## CI Usage

Add the same script or command as a dedicated CI step to enforce regression
guardrails on representative hardware.
