spec homer

# Design

## Area 0: init
files:
  - homer-cli/src/commands/init.rs

## Area 1: mod
files:
  - homer-cli/src/commands/mod.rs

## Area 2: status
files:
  - homer-cli/src/commands/status.rs

## Area 3: update
files:
  - homer-cli/src/commands/update.rs

## Area 4: main
files:
  - homer-cli/src/main.rs

## Area 5: behavioral
files:
  - homer-core/src/analyze/behavioral.rs

## Area 6: config
files:
  - homer-core/src/config.rs

## Area 7: mod
files:
  - homer-core/src/store/mod.rs

## Area 8: types
files:
  - homer-core/src/types.rs

## Area 9: sqlite
files:
  - homer-core/src/store/sqlite.rs

## Area 10: mod
files:
  - homer-core/src/analyze/mod.rs

## Area 11: traits
files:
  - homer-core/src/analyze/traits.rs

## Area 12: document
files:
  - homer-core/src/extract/document.rs

## Area 13: src
files:
  - homer-core/src/extract/traits.rs
  - homer-core/src/error.rs

## Area 14: git
files:
  - homer-core/src/extract/git.rs

## Area 15: graph
files:
  - homer-core/src/extract/graph.rs

## Area 16: structure
files:
  - homer-core/src/extract/structure.rs

## Area 17: mod
files:
  - homer-core/src/llm/mod.rs

## Area 18: pipeline
files:
  - homer-core/src/pipeline.rs

## Area 19: centrality
files:
  - homer-core/src/analyze/centrality.rs

## Area 20: community
files:
  - homer-core/src/analyze/community.rs

## Area 21: convention
files:
  - homer-core/src/analyze/convention.rs

## Area 22: task_pattern
files:
  - homer-core/src/analyze/task_pattern.rs

## Area 23: temporal
files:
  - homer-core/src/analyze/temporal.rs

## Area 24: github
files:
  - homer-core/src/extract/github.rs

## Area 25: gitlab
files:
  - homer-core/src/extract/gitlab.rs

## Area 26: prompt
files:
  - homer-core/src/extract/prompt.rs

## Area 27: progress
files:
  - homer-core/src/progress.rs

## Area 28: agents_md
files:
  - homer-core/src/render/agents_md.rs

## Area 29: module_context
files:
  - homer-core/src/render/module_context.rs

## Area 30: report
files:
  - homer-core/src/render/report.rs

## Area 31: risk_map
files:
  - homer-core/src/render/risk_map.rs

## Area 32: skills
files:
  - homer-core/src/render/skills.rs

## Area 33: topos_spec
files:
  - homer-core/src/render/topos_spec.rs

## Area 34: traits
files:
  - homer-core/src/render/traits.rs

## Area 35: incremental
files:
  - homer-core/src/store/incremental.rs

## Area 36: schema
files:
  - homer-core/src/store/schema.rs

## Area 37: traits
files:
  - homer-core/src/store/traits.rs

## Area 38: call_graph
files:
  - homer-graphs/src/call_graph.rs

## Area 39: scope_graph
files:
  - homer-graphs/src/scope_graph.rs

## Area 40: diff
files:
  - homer-graphs/src/diff.rs

## Area 41: import_graph
files:
  - homer-graphs/src/import_graph.rs

## Area 42: fallback
files:
  - homer-graphs/src/languages/fallback.rs

## Area 43: go
files:
  - homer-graphs/src/languages/go.rs

## Area 44: java
files:
  - homer-graphs/src/languages/java.rs

## Area 45: javascript
files:
  - homer-graphs/src/languages/javascript.rs

## Area 46: ARCHITECTURE
files:
  - homer-spec/ARCHITECTURE.md
  - homer-graphs/src/languages/mod.rs

## Area 47: python
files:
  - homer-graphs/src/languages/python.rs

## Area 48: rust
files:
  - homer-graphs/src/languages/rust.rs

## Area 49: typescript
files:
  - homer-graphs/src/languages/typescript.rs

## Area 50: lib
files:
  - homer-graphs/src/lib.rs

## Area 51: lib
files:
  - homer-mcp/src/lib.rs

## Area 52: lib
files:
  - homer-test/src/lib.rs

## Area 53: helpers
files:
  - homer-graphs/src/languages/helpers.rs

## Area 54: pipeline
files:
  - homer-test/tests/pipeline.rs

## Area 55: graph
files:
  - homer-cli/src/commands/graph.rs

## Area 56: query
files:
  - homer-cli/src/commands/query.rs

## Area 57: cache
files:
  - homer-core/src/llm/cache.rs

## Area 58: providers
files:
  - homer-core/src/llm/providers.rs

## Area 59: diff
files:
  - homer-cli/src/commands/diff.rs

## Area 60: semantic
files:
  - homer-core/src/analyze/semantic.rs

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

## Area 65: ecma_scope
files:
  - homer-graphs/src/languages/ecma_scope.rs

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
