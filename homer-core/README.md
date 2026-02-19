# homer-core

The core library for Homer. Contains the pipeline orchestrator, data store, extractors, analyzers, renderers, and LLM client.

## Key Modules

- **`pipeline`** — `HomerPipeline` orchestrates Extract → Auto Snapshots → Analyze → Render. Topologically sorts analyzers via `produces()`/`requires()` declarations. Fault-tolerant: individual stage failures are collected, not propagated.

- **`store/`** — `HomerStore` trait and `SqliteStore` implementation. WAL mode, content-hash-based upserts, FTS5 search, snapshot management. All pipeline stages read/write through this trait.

- **`extract/`** — 7 extractors populate the hypergraph:
  - `git` — Commit history, contributors, releases (via `gix`)
  - `structure` — File tree, modules, external dependencies
  - `graph` — Functions, types, calls, imports (via `homer-graphs`)
  - `document` — README, ADR, and documentation files
  - `github` — Pull requests, issues, reviews (GitHub API)
  - `gitlab` — Merge requests, issues, reviews (GitLab API)
  - `prompt` — AI agent interactions and rules

- **`analyze/`** — 7 analyzers compute derived insights:
  - `behavioral` — Change frequency, churn, bus factor, co-changes
  - `centrality` — PageRank, betweenness, HITS, composite salience
  - `community` — Louvain community detection
  - `temporal` — Centrality trends, architectural drift, stability
  - `convention` — Naming, testing, error handling patterns
  - `task_pattern` — Recurring development patterns, domain vocabulary
  - `semantic` — LLM-powered summaries, rationale, invariants

- **`render/`** — 6 renderers produce output artifacts:
  - `agents_md` — `AGENTS.md` for AI coding agents
  - `module_context` — Per-directory `.context.md` files
  - `risk_map` — `homer-risk.json` for CI pipelines
  - `skills` — Claude Code skill files
  - `topos_spec` — Topological specification files
  - `report` — Human-readable HTML/Markdown report

- **`types`** — Exhaustive enums: `NodeKind` (15), `HyperedgeKind` (17), `AnalysisKind` (25), `SalienceClass` (4), `StabilityClass` (4). All `Copy`.

- **`config`** — `HomerConfig` with depth overrides, per-renderer config, invalidation policy.

- **`llm/`** — Provider abstraction (Anthropic, OpenAI, custom), caching, cost budgets.

## Entry Point

```rust
use homer_core::pipeline::HomerPipeline;
use homer_core::store::sqlite::SqliteStore;
use homer_core::config::HomerConfig;

let store = SqliteStore::open(Path::new(".homer/homer.db"))?;
let config = HomerConfig::default().with_depth_overrides();
let pipeline = HomerPipeline::new(Path::new("."));
let result = pipeline.run(&store, &config).await?;
```

## Tests

223 unit tests including proptest round-trips for all types and store operations.
