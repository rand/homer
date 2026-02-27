# <!-- homer:generated at=2026-02-27T21:47:53Z commit=f54ad32 -->
spec homer

# Principles

- ADR 0001: Canonical Edge Roles and Analysis Keys

- ADR 0002: Deterministic Hyperedge Identity for Idempotent Upserts

- ADR 0003: MCP Transport Strategy — Stdio Only

# Design

## Area 0: behavioral
files:
  - homer-core/src/analyze/behavioral.rs
  - homer-cli/src/commands/init.rs
  - homer-core/src/render/agents_md.rs
  - homer-core/src/analyze/traits.rs

## Area 1: mod
files:
  - homer-core/src/store/mod.rs
  - homer-cli/src/commands/mod.rs
  - homer-core/src/extract/forge_common.rs
  - homer-core/src/query/mod.rs

## Area 2: github
files:
  - homer-core/src/extract/github.rs
  - homer-cli/src/commands/status.rs
  - homer-core/src/config.rs
  - homer-core/src/render/traits.rs

## Area 3: risk_map
files:
  - homer-core/src/render/risk_map.rs
  - homer-core/src/extract/prompt.rs
  - homer-core/src/store/sqlite.rs
  - homer-cli/src/commands/update.rs

## Area 4: types
files:
  - homer-core/src/types.rs
  - homer-core/src/render/skills.rs
  - homer-core/src/store/traits.rs
  - homer-cli/src/main.rs

## Area 5: document
files:
  - homer-core/src/extract/document.rs

## Area 6: src
files:
  - homer-core/src/extract/git.rs
  - homer-core/src/extract/graph.rs
  - homer-core/src/extract/structure.rs
  - homer-core/src/extract/gitlab.rs
  - homer-core/src/error.rs

## Area 7: src
files:
  - homer-core/src/analyze/convention.rs
  - homer-core/src/extract/traits.rs

## Area 8: src
files:
  - homer-core/src/llm/providers.rs
  - homer-core/src/progress.rs
  - homer-core/src/pipeline.rs

## Area 9: src
files:
  - homer-core/src/analyze/task_pattern.rs
  - homer-core/src/store/incremental.rs
  - homer-core/src/analyze/centrality.rs
  - homer-core/src/analyze/community.rs

## Area 10: src
files:
  - homer-core/src/llm/mod.rs
  - homer-core/src/analyze/mod.rs
  - homer-core/src/llm/cache.rs
  - homer-core/src/analyze/semantic.rs

## Area 11: src
files:
  - homer-core/src/analyze/temporal.rs
  - homer-core/src/render/report.rs
  - homer-core/src/render/module_context.rs
  - homer-core/src/contracts.rs

## Area 12: topos_spec
files:
  - homer-core/src/render/topos_spec.rs

## Area 13: src
files:
  - homer-graphs/src/languages/java.rs
  - homer-graphs/src/scope_graph.rs
  - homer-graphs/src/languages/ecma_scope.rs
  - homer-graphs/src/import_graph.rs
  - homer-graphs/src/languages/typescript.rs
  - homer-graphs/src/languages/go.rs
  - homer-graphs/src/languages/python.rs
  - homer-graphs/src/languages/helpers.rs
  - homer-graphs/src/call_graph.rs
  - homer-graphs/src/languages/javascript.rs
  # ... and 8 more

## Area 14: lib
files:
  - homer-mcp/src/lib.rs
  - homer-spec/ARCHITECTURE.md
  - homer-graphs/src/languages/mod.rs

## Area 15: ANALYZERS
files:
  - homer-spec/ANALYZERS.md
  - homer-spec/EXTRACTORS.md
  - homer-core/tests/perf_regression.rs

## Area 36: schema
files:
  - homer-core/src/store/schema.rs

## Area 40: diff
files:
  - homer-graphs/src/diff.rs

## Area 42: fallback
files:
  - homer-graphs/src/languages/fallback.rs

## Area 50: lib
files:
  - homer-graphs/src/lib.rs

## Area 52: lib
files:
  - homer-test/src/lib.rs

## Area 54: pipeline
files:
  - homer-test/tests/pipeline.rs

## Area 55: graph
files:
  - homer-cli/src/commands/graph.rs

## Area 56: query
files:
  - homer-cli/src/commands/query.rs

## Area 59: diff
files:
  - homer-cli/src/commands/diff.rs

## Area 61: serve
files:
  - homer-cli/src/commands/serve.rs

## Area 62: centrality_bench
files:
  - homer-core/benches/centrality_bench.rs

## Area 63: parse_bench
files:
  - homer-core/benches/parse_bench.rs

## Area 64: store_bench
files:
  - homer-core/benches/store_bench.rs

## Area 67: render
files:
  - homer-cli/src/commands/render.rs

## Area 68: risk_check
files:
  - homer-cli/src/commands/risk_check.rs

## Area 69: snapshot
files:
  - homer-cli/src/commands/snapshot.rs

# Concepts

Concept InitArgs:
  file: homer-cli/src/commands/init.rs

Concept diff:
  file: homer-cli/src/commands/mod.rs

Concept graph:
  file: homer-cli/src/commands/mod.rs

Concept init:
  file: homer-cli/src/commands/mod.rs

Concept query:
  file: homer-cli/src/commands/mod.rs

Concept serve:
  file: homer-cli/src/commands/mod.rs

Concept status:
  file: homer-cli/src/commands/mod.rs

Concept update:
  file: homer-cli/src/commands/mod.rs

Concept Command:
  file: homer-cli/src/commands/mod.rs

Concept StatusArgs:
  file: homer-cli/src/commands/status.rs

Concept UpdateArgs:
  file: homer-cli/src/commands/update.rs

Concept commands:
  file: homer-cli/src/main.rs

Concept Cli:
  file: homer-cli/src/main.rs

Concept BehavioralAnalyzer:
  file: homer-core/src/analyze/behavioral.rs

Concept CommitData:
  file: homer-core/src/analyze/behavioral.rs
  description: Intermediate data collected from the store for analysis.

Concept FileChange:
  file: homer-core/src/analyze/behavioral.rs

Concept CoChangeConfig:
  file: homer-core/src/analyze/behavioral.rs
  description: Configuration for co-change detection (per ANALYZERS.md spec).

Concept ScoredPair:
  file: homer-core/src/analyze/behavioral.rs

Concept tests:
  file: homer-core/src/analyze/behavioral.rs

Concept behavioral:
  file: homer-core/src/analyze/mod.rs
