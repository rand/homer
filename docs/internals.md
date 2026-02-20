# Internals

A guide to Homer's architecture for contributors. Read this if you want to understand how the code works, not just what it does.

## Crate Architecture

Homer is a Cargo workspace with 5 crates:

```
homer-core/     Pipeline orchestration, store, extractors, analyzers, renderers, LLM client
homer-graphs/   Tree-sitter extraction, scope graphs, language support (6 languages)
homer-cli/      The `homer` binary (clap-based CLI, 10 commands)
homer-mcp/      MCP server (rmcp, 5 tools)
homer-test/     Integration tests and fixture repos
```

Dependencies flow downward: `homer-cli` and `homer-mcp` depend on `homer-core`, which depends on `homer-graphs`. `homer-test` depends on `homer-core`.

## Pipeline Orchestration

**File:** `homer-core/src/pipeline.rs`

`HomerPipeline` orchestrates the full Extract → Auto Snapshots → Analyze → Render flow. Key design decisions:

### Fault Tolerance

Every stage collects errors in `PipelineResult.errors` without aborting. A broken git repo doesn't prevent structure extraction; a failed renderer doesn't block other renderers. This is critical because real repositories are messy — partial results are better than no results.

```rust
// Each extractor/analyzer/renderer gets its own error handling
match ext.extract(store, config).await {
    Ok(stats) => { /* accumulate */ },
    Err(e) => { result.errors.push(PipelineError { stage, message }); }
}
```

### Topological Sort

Analyzers declare what they `produces()` and `requires()` via `AnalysisKind` slices. The pipeline uses Kahn's algorithm to sort them into a valid execution order:

1. Build a producer map: `AnalysisKind → analyzer index`
2. Build an adjacency list from `requires()` declarations
3. BFS from zero-in-degree nodes
4. If cycles exist (should never happen), append remaining analyzers as a fallback

The current order is: behavioral → centrality → community → temporal → convention → task pattern → semantic.

### Auto Snapshots

Between extraction and analysis, the pipeline creates graph snapshots based on `[graph.snapshots]` config. This happens after extraction so the snapshot captures the latest state. Two trigger types:

- **Release-triggered**: Creates a snapshot for each `NodeKind::Release` node not yet snapshotted
- **Commit-count-triggered**: Creates a snapshot when the commit count exceeds the threshold since the last `auto-*` snapshot

### Progress Reporting

The pipeline accepts a `&dyn ProgressReporter` for user-visible feedback. The CLI passes a spinner implementation; tests use `NoopReporter`. The trait is simple:

```rust
pub trait ProgressReporter: Send + Sync {
    fn start(&self, message: &str, total: Option<u64>);
    fn message(&self, text: &str);
    fn finish(&self);
}
```

## Store Layer

**Files:** `homer-core/src/store/{traits.rs, sqlite.rs, schema.rs, incremental.rs}`

### The `HomerStore` Trait

All pipeline stages read and write through the `HomerStore` trait (`async_trait`). This allows testing with in-memory SQLite and makes the store implementation swappable. Key operations:

- **Node CRUD**: `upsert_node`, `get_node`, `get_node_by_name`, `find_nodes`, batch upsert
- **Edge CRUD**: `upsert_hyperedge`, `get_edges_involving`, `get_edges_by_kind`
- **Analysis**: `store_analysis`, `get_analysis`, `get_analyses_by_kind`, `clear_analyses_by_kind`
- **Graph loading**: `load_subgraph` with `SubgraphFilter` (full, neighborhood, high-salience, module, by-kind, intersection)
- **Snapshots**: `create_snapshot`, `list_snapshots`, `diff_snapshots`
- **Checkpoints**: `set_checkpoint`, `get_checkpoint` (key-value pairs for incrementality)
- **Search**: `search_nodes` with `SearchScope` (full-text search via FTS5)
- **Stats**: `get_stats` (total counts, size)
- **Aliasing**: `resolve_canonical`, `alias_chain` (entity rename tracking)

### SQLite Implementation

`SqliteStore` uses `rusqlite` with WAL mode for concurrent reads during pipeline execution. Notable details:

**Content hashing**: Rust uses `u64` for content hashes, but SQLite's integer type is signed `i64`. Homer uses bit reinterpretation (not truncation) to store `u64` as `i64`:

```rust
let stored = hash as i64;        // bit-reinterpret u64 → i64
let recovered = stored as u64;   // bit-reinterpret back
```

**Upsert semantics**: Nodes are identified by `(kind, name)`. The `upsert_node` method uses `INSERT OR REPLACE` with content hash comparison — if the hash hasn't changed, the node is not updated.

**Batch operations**: `upsert_nodes_batch` wraps multiple inserts in a single transaction for 10-100x throughput improvement on large extractions.

**Schema**: Defined in `schema.rs` with `CREATE TABLE IF NOT EXISTS` statements. Tables: `nodes`, `hyperedges` (with deterministic `identity_key`), `hyperedge_members`, `analysis_results`, `checkpoints`, `snapshots`, `snapshot_nodes`, `snapshot_edges`, `nodes_fts` (FTS5).

### Incrementality

The `incremental.rs` module manages checkpoint-based incrementality:

- **Git extractor**: Stores `git_last_sha` checkpoint. On update, only processes commits after this SHA.
- **Extractor checkpoints**: Structure/document/prompt extractors store `*_last_sha` checkpoints and skip when unchanged.
- **Changed-file graph extraction**: Graph extractor tracks `graph_last_sha` and scopes parsing to files changed since that checkpoint.
- **Idempotent edges**: Hyperedges are upserted by deterministic semantic identity.
- **Analysis invalidation**: Controlled by `[analysis.invalidation]` config. Centrality scores are invalidated globally on topology changes; semantic summaries only on direct content changes.

## Graph Engine

**Files:** `homer-graphs/src/{scope_graph.rs, call_graph.rs, import_graph.rs, diff.rs}`

### Tree-sitter Integration

Homer uses tree-sitter 0.25 for parsing. Each language provides its grammar via a crate (e.g., `tree-sitter-rust`). The `LanguageSupport` trait dispatches to language-specific extractors:

```rust
pub trait LanguageSupport: Send + Sync {
    fn id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn tier(&self) -> ResolutionTier;
    fn tree_sitter_language(&self) -> tree_sitter::Language;
    fn build_scope_graph(&self, tree, source, path) -> Result<Option<FileScopeGraph>>;
    fn extract_heuristic(&self, tree, source, path) -> Result<HeuristicGraph>;
}
```

### Scope Graph Construction

All 6 languages use `ResolutionTier::Precise` via scope graph construction. A scope graph maps every definition and reference to a scope, enabling accurate cross-file resolution:

1. **Parse** the file with tree-sitter
2. **Walk** the AST, creating scope nodes for modules, functions, blocks
3. **Record** definitions (function defs, type defs, imports) and references (call sites, type refs)
4. **Resolve** references to definitions within the file's scope hierarchy

The `ScopeGraphBuilder` helper (in `languages/helpers.rs`) provides common scope graph operations used by all language implementations.

### Language Dispatch

`homer-graphs/src/languages/mod.rs` maps file extensions to `LanguageSupport` implementations. Language detection happens in `GraphExtractor` based on file extensions and the `[graph.languages]` config.

TypeScript and JavaScript share a common ECMAScript scope graph walker (`ecma_scope.rs`) to avoid duplication.

## Type System

**File:** `homer-core/src/types.rs`

Homer's type system is built on exhaustive enums:

- `NodeKind` — 15 variants (File, Function, Type, Module, Commit, PullRequest, Issue, Contributor, Release, Concept, ExternalDep, Document, Prompt, AgentRule, AgentSession)
- `HyperedgeKind` — 17 variants
- `AnalysisKind` — 25 variants
- `SalienceClass` — 4 variants (ActiveHotspot, FoundationalStable, PeripheralActive, QuietLeaf)
- `StabilityClass` — 4 variants (StableCore, ActiveCore, StableLeaf, ActiveLeaf)

All are `Copy` types. Using exhaustive enums instead of strings means the compiler enforces completeness — adding a new `NodeKind` variant produces errors everywhere that needs updating. This is intentional: the compile-time cost is worth the guarantee that nothing is forgotten.

`AnalysisKind` in particular serves as the dependency declaration system for analyzer topological sorting — each analyzer declares which kinds it `produces()` and `requires()`.

## Config System

**File:** `homer-core/src/config.rs`

`HomerConfig` is a serde-driven TOML structure with `#[serde(default)]` on all sections. This means partial TOML files work — any unspecified field gets its default value.

### Depth Overrides

`HomerConfig::with_depth_overrides()` applies the depth table to extraction and analysis settings. This is called after loading the config from disk, so the TOML file stores the user's intent while the runtime config reflects the depth-adjusted values.

### Language Config

`LanguageConfig` has a custom serde implementation: it deserializes either the string `"auto"` or an array of language names like `["rust", "python"]`.

## LLM Integration

**Files:** `homer-core/src/llm/{mod.rs, providers.rs, cache.rs}`

### Provider Abstraction

The `LlmProvider` trait abstracts over LLM providers (Anthropic, OpenAI, custom). The `create_provider` factory function constructs the right provider based on config.

### Cost Budget

The LLM client tracks cumulative cost per run. When `cost_budget` is set (> 0), requests are rejected once the budget is exceeded. This prevents runaway API costs during large analyses.

### Caching

LLM responses are cached by a hash of the prompt + entity content hash. If the entity hasn't changed since the last run, the cached response is reused without an API call. The `--force-semantic` flag clears this cache.

## Analyzer Pipeline

Each analyzer implements the `Analyzer` trait:

```rust
#[async_trait]
pub trait Analyzer: Send + Sync {
    fn name(&self) -> &'static str;
    fn produces(&self) -> &'static [AnalysisKind] { &[] }
    fn requires(&self) -> &'static [AnalysisKind] { &[] }
    async fn needs_rerun(&self, store: &dyn HomerStore) -> Result<bool> { Ok(true) }
    async fn analyze(&self, store: &dyn HomerStore, config: &HomerConfig) -> Result<AnalyzeStats>;
}
```

The `produces()`/`requires()` declarations are the key innovation: they make analyzer dependencies explicit and compiler-checked. Adding a new `AnalysisKind` that an existing analyzer should consume is a compile error until you update the `requires()` list.

### Analyzer → AnalysisKind Mapping

| Analyzer | Produces |
|----------|----------|
| Behavioral | ChangeFrequency, ChurnVelocity, ContributorConcentration, DocumentationCoverage, DocumentationFreshness, PromptHotspot, CorrectionHotspot |
| Centrality | PageRank, BetweennessCentrality, HITSScore, CompositeSalience |
| Community | CommunityAssignment |
| Temporal | CentralityTrend, ArchitecturalDrift, StabilityClassification |
| Convention | NamingPattern, TestingPattern, ErrorHandlingPattern, DocumentationStylePattern, AgentRuleValidation |
| Task Pattern | TaskPattern, DomainVocabulary |
| Semantic | SemanticSummary, DesignRationale, InvariantDescription |

## MCP Server

**File:** `homer-mcp/src/lib.rs`

The MCP server uses the `rmcp` crate. The `#[tool_router]` and `#[tool]` macros generate the MCP tool definitions from Rust structs. The server is backed by an `Arc<SqliteStore>` for thread-safe access.

Each tool method (`do_query`, `do_graph`, etc.) is separated from the MCP dispatch for testability — unit tests call `do_query` directly without MCP transport.

## Testing Patterns

- **Unit tests**: `#[cfg(test)]` modules in each source file
- **Property tests**: `proptest` for serde round-trips (types, store operations)
- **Integration tests**: `homer-test/tests/pipeline.rs` — 14 tests covering end-to-end pipeline, auto snapshots, empty repos
- **MCP tests**: In-memory store tests for each MCP tool
- **Benchmarks**: Criterion benchmarks in `homer-core/benches/` (store, parse, centrality)

Total: 357 tests (223 core + 102 graphs + 7 MCP + 11 CLI + 14 integration).

## Next Steps

- [Extending Homer](extending.md) — Step-by-step guides for adding features
- [Concepts](concepts.md) — User-facing explanation of how Homer works
- [Configuration](configuration.md) — Full config reference
