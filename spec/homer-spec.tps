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

## Area 6: mod
files:
  - homer-core/src/analyze/mod.rs

## Area 7: traits
files:
  - homer-core/src/analyze/traits.rs

## Area 8: config
files:
  - homer-core/src/config.rs

## Area 9: document
files:
  - homer-core/src/extract/document.rs

## Area 10: git
files:
  - homer-core/src/extract/git.rs

## Area 11: graph
files:
  - homer-core/src/extract/graph.rs

## Area 12: structure
files:
  - homer-core/src/extract/structure.rs

## Area 13: traits
files:
  - homer-core/src/extract/traits.rs

## Area 14: mod
files:
  - homer-core/src/llm/mod.rs

## Area 15: pipeline
files:
  - homer-core/src/pipeline.rs

## Area 16: agents_md
files:
  - homer-core/src/render/agents_md.rs

## Area 17: traits
files:
  - homer-core/src/render/traits.rs

## Area 18: mod
files:
  - homer-core/src/store/mod.rs

## Area 19: schema
files:
  - homer-core/src/store/schema.rs

## Area 20: sqlite
files:
  - homer-core/src/store/sqlite.rs

## Area 21: traits
files:
  - homer-core/src/store/traits.rs

## Area 22: types
files:
  - homer-core/src/types.rs

## Area 23: call_graph
files:
  - homer-graphs/src/call_graph.rs

## Area 24: diff
files:
  - homer-graphs/src/diff.rs

## Area 25: import_graph
files:
  - homer-graphs/src/import_graph.rs

## Area 26: fallback
files:
  - homer-graphs/src/languages/fallback.rs

## Area 27: go
files:
  - homer-graphs/src/languages/go.rs

## Area 28: java
files:
  - homer-graphs/src/languages/java.rs

## Area 29: javascript
files:
  - homer-graphs/src/languages/javascript.rs

## Area 30: mod
files:
  - homer-graphs/src/languages/mod.rs

## Area 31: python
files:
  - homer-graphs/src/languages/python.rs

## Area 32: rust
files:
  - homer-graphs/src/languages/rust.rs

## Area 33: typescript
files:
  - homer-graphs/src/languages/typescript.rs

## Area 34: lib
files:
  - homer-graphs/src/lib.rs

## Area 35: scope_graph
files:
  - homer-graphs/src/scope_graph.rs

## Area 36: lib
files:
  - homer-mcp/src/lib.rs

## Area 37: lib
files:
  - homer-test/src/lib.rs

## Area 38: helpers
files:
  - homer-graphs/src/languages/helpers.rs

## Area 39: pipeline
files:
  - homer-test/tests/pipeline.rs

## Area 40: centrality
files:
  - homer-core/src/analyze/centrality.rs

## Area 41: community
files:
  - homer-core/src/analyze/community.rs

## Area 42: graph
files:
  - homer-cli/src/commands/graph.rs

## Area 43: query
files:
  - homer-cli/src/commands/query.rs

## Area 44: module_context
files:
  - homer-core/src/render/module_context.rs

## Area 45: risk_map
files:
  - homer-core/src/render/risk_map.rs

## Area 46: convention
files:
  - homer-core/src/analyze/convention.rs

## Area 47: github
files:
  - homer-core/src/extract/github.rs

## Area 48: cache
files:
  - homer-core/src/llm/cache.rs

## Area 49: providers
files:
  - homer-core/src/llm/providers.rs

## Area 50: diff
files:
  - homer-cli/src/commands/diff.rs

## Area 51: semantic
files:
  - homer-core/src/analyze/semantic.rs

## Area 52: serve
files:
  - homer-cli/src/commands/serve.rs

## Area 53: task_pattern
files:
  - homer-core/src/analyze/task_pattern.rs

## Area 54: prompt
files:
  - homer-core/src/extract/prompt.rs

## Area 55: centrality_bench
files:
  - homer-core/benches/centrality_bench.rs

## Area 56: parse_bench
files:
  - homer-core/benches/parse_bench.rs

## Area 57: store_bench
files:
  - homer-core/benches/store_bench.rs

## Area 58: gitlab
files:
  - homer-core/src/extract/gitlab.rs

## Area 59: topos_spec
files:
  - homer-core/src/render/topos_spec.rs

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

Concept tests:
  file: homer-core/src/analyze/behavioral.rs

Concept behavioral:
  file: homer-core/src/analyze/mod.rs

Concept centrality:
  file: homer-core/src/analyze/mod.rs

Concept community:
  file: homer-core/src/analyze/mod.rs
