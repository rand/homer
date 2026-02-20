#!/usr/bin/env bash
set -euo pipefail

# Optional threshold overrides (milliseconds):
#   HOMER_PERF_STRUCTURE_MS
#   HOMER_PERF_PARSE_MS
#   HOMER_PERF_CENTRALITY_MS
#   HOMER_PERF_INCREMENTAL_MS

cargo test -p homer-core --test perf_regression -- --ignored --nocapture
