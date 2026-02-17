# Homer Architecture

> System architecture, pipeline design, and crate structure.

**Parent**: [README.md](README.md)  
**Related**: [STORE.md](STORE.md) · [EXTRACTORS.md](EXTRACTORS.md) · [ANALYZERS.md](ANALYZERS.md) · [RENDERERS.md](RENDERERS.md) · [PERFORMANCE.md](PERFORMANCE.md)

---

## Pipeline Overview

Homer operates as a four-stage pipeline with a persistent hypergraph store at the center:

```
┌────────────┐    ┌────────────┐    ┌──────────────┐    ┌────────────┐
│ Extractors │ ─→ │  Store     │ ─→ │  Analyzers   │ ─→ │ Renderers  │
│ (data in)  │    │ (IR/state) │    │  (insights)  │    │ (artifacts)│
└────────────┘    └──────┬─────┘    └──────────────┘    └────────────┘
                         │                                      │
                         │         ┌──────────────┐             │
                         └────────→│ Query Engine │←────────────┘
                                   └──────────────┘
                                          │
                                   ┌──────┴──────┐
                                   │  CLI / MCP  │
                                   └─────────────┘
```

### Stage 1: Extraction (deterministic, incremental)

Extractors pull raw data from the repository and its hosting platform. Each extractor tracks its own checkpoint (last-processed commit SHA, last-processed PR number, content hash, timestamp) and only processes new data on subsequent runs. All extractors write to the [hypergraph store](STORE.md).

See [EXTRACTORS.md](EXTRACTORS.md) for full specification.

| Extractor | Input | Output | Incrementality |
|-----------|-------|--------|----------------|
| Git History | `.git` directory | Commits, diffs, authors, tags | Last processed SHA |
| GitHub API | GitHub REST/GraphQL | PRs, issues, reviews, comments | Last PR/issue number |
| Structure | File tree | Directory layout, configs, manifests | File modification time |
| Graph | Source files | Call graphs, import graphs, doc comments | Per-file content hash |
| Documents | Markdown, RST, .tps files | Document nodes, cross-references | Content hash per file |
| Prompts | .claude/, .cursor/rules, .beads/ | Prompts, sessions, agent rules, task patterns | Timestamp per source |

### Stage 2: Storage (hypergraph persistence)

The [hypergraph store](STORE.md) is the heart of Homer. It persists all extracted data, computed metrics, and cached analysis results in a SQLite database. The hypergraph model supports n-ary relationships (hyperedges connecting multiple nodes), which naturally represent concepts like "commit C modified files {F1, F2, F3}" and "files {A, B, C} always co-change."

The store also manages incrementality metadata: freshness timestamps, content hashes, and invalidation cascades.

### Stage 3: Analysis (algorithmic core + selective LLM)

Analyzers read from the store, compute derived insights, and write results back. Analysis is tiered by computational cost:

See [ANALYZERS.md](ANALYZERS.md) for full specification.

| Analyzer | Type | Cost | Depends On |
|----------|------|------|------------|
| Behavioral | Algorithmic | Low | Git history, documents, prompts |
| Centrality | Algorithmic | Medium | Call/import graphs |
| Temporal | Algorithmic | Medium | Graph snapshots over time |
| Convention | Heuristic + LLM | Medium | ASTs, identifiers, patterns, agent rules |
| Task Pattern | Algorithmic + LLM | Medium | Prompt extraction data |
| Semantic | LLM-powered | High | High-salience nodes (gated), doc comments |

**Critical design principle**: The graph analysis (centrality, community detection) runs *first* and is purely algorithmic. It identifies which entities are high-salience. Only those entities get LLM summarization. This gates the expensive operation behind a cheap filter. Additionally, high-salience entities with good doc comments can skip or simplify LLM summarization — the doc comment *is* the human-authored summary.

### Stage 4: Rendering (artifact generation)

Renderers read from the store (both raw data and analysis results) and produce output files. Each renderer is independent and can be run selectively.

See [RENDERERS.md](RENDERERS.md) for full specification.

| Renderer | Output | Primary Consumer |
|----------|--------|-----------------|
| AGENTS.md | `AGENTS.md` / `CLAUDE.md` | AI coding agents |
| Module Context | Per-directory `.context.md` files | AI agents (scoped) |
| Skills | Claude Code skill files | Claude Code |
| Spec | `.tps` (Topos format) specification | Humans + Topos tooling |
| Report | HTML/Markdown report with visualizations | Humans |
| Risk Map | JSON risk annotations | AI agents (guardrails) |

### Query Engine

The query engine provides read access to the hypergraph store for interactive use (CLI queries) and programmatic access (MCP tools). It supports:

- Entity lookup: "What does Homer know about this file/function?"
- Graph traversal: "What depends on this function?" "What does this module import?"
- Metric queries: "Top N by PageRank/betweenness/composite salience"
- Temporal queries: "How has this function's centrality changed over the last 6 months?"
- Text search: Full-text search over commit messages, PR descriptions, semantic summaries, documentation
- Document queries: "What documentation covers this module?"
- Prompt queries: "What tasks have agents performed in this area?"

---

## Crate Structure

Homer is organized as a Cargo workspace:

```
homer/
├── Cargo.toml                    # Workspace root
├── homer-core/                   # Core library — all pipeline logic
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # Public API surface
│       ├── extract/              # Extractors
│       │   ├── mod.rs
│       │   ├── git.rs            # Git history (libgit2 via git2 crate)
│       │   ├── github.rs         # GitHub API (reqwest + octocrab)
│       │   ├── structure.rs      # File tree, configs, manifests
│       │   ├── graph.rs          # Scope graph construction orchestrator
│       │   ├── document.rs       # Documentation file extraction
│       │   └── prompt.rs         # AI agent interaction extraction
│       ├── analyze/              # Analyzers
│       │   ├── mod.rs
│       │   ├── behavioral.rs     # Change frequency, co-change, churn, doc coverage
│       │   ├── centrality.rs     # PageRank, betweenness, HITS
│       │   ├── community.rs      # Community detection (Louvain/Leiden)
│       │   ├── temporal.rs       # Graph evolution, drift detection
│       │   ├── semantic.rs       # LLM-powered summaries (doc-comment-aware)
│       │   ├── convention.rs     # Pattern extraction (validates agent rules)
│       │   └── task_pattern.rs   # Prompt-derived task pattern extraction
│       ├── store/                # Hypergraph persistence
│       │   ├── mod.rs
│       │   ├── schema.rs         # SQLite schema definitions + migrations
│       │   ├── hypergraph.rs     # Core data structures
│       │   ├── sqlite.rs         # rusqlite implementation
│       │   ├── traits.rs         # HomerStore trait definition
│       │   └── incremental.rs    # Freshness tracking, invalidation
│       ├── render/               # Artifact renderers
│       │   ├── mod.rs
│       │   ├── agents_md.rs      # AGENTS.md / CLAUDE.md
│       │   ├── module_ctx.rs     # Per-directory context
│       │   ├── skills.rs         # Claude Code skills
│       │   ├── spec.rs           # Topos-format specification
│       │   ├── report.rs         # Human-readable report
│       │   └── risk_map.rs       # Risk annotations
│       ├── query/                # Query engine
│       │   ├── mod.rs
│       │   ├── entity.rs         # Entity lookup
│       │   ├── graph.rs          # Graph traversal queries
│       │   ├── metrics.rs        # Metric queries
│       │   └── search.rs         # Full-text search
│       └── llm/                  # LLM integration
│           ├── mod.rs
│           ├── client.rs         # HTTP client (reqwest)
│           ├── prompt.rs         # Prompt templates
│           └── providers.rs      # Anthropic, OpenAI, etc.
│
├── homer-graphs/                 # Graph extraction engine
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── scope_graph.rs        # Core scope graph data structures
│       ├── path_stitching.rs     # Name resolution algorithm
│       ├── call_graph.rs         # Call graph projection from scope graph
│       ├── import_graph.rs       # Import graph extraction
│       ├── diff.rs               # Graph diffing between snapshots
│       └── languages/            # Per-language support
│           ├── mod.rs            # LanguageSupport trait + registry
│           ├── python.rs         # Python TSG rules
│           ├── typescript.rs     # TypeScript TSG rules
│           ├── javascript.rs     # JavaScript TSG rules
│           ├── java.rs           # Java TSG rules
│           ├── rust.rs           # Rust TSG rules (new)
│           ├── go.rs             # Go TSG rules (new)
│           └── fallback.rs       # Tree-sitter heuristic extraction
│
├── homer-cli/                    # CLI binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── commands/
│       │   ├── init.rs
│       │   ├── update.rs
│       │   ├── render.rs
│       │   ├── query.rs
│       │   ├── graph.rs
│       │   ├── diff.rs
│       │   └── serve.rs
│       └── config.rs             # Configuration loading
│
├── homer-mcp/                    # MCP server
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── tools.rs              # MCP tool definitions
│
└── homer-test/                   # Integration tests + fixtures
    ├── Cargo.toml
    ├── fixtures/                 # Sample repositories for testing
    └── src/
        └── lib.rs
```

### Dependency Graph Between Crates

```
homer-cli ──→ homer-core ──→ homer-graphs
    │              │
    └──→ homer-mcp─┘
              │
              └──→ homer-core
```

- `homer-graphs`: No dependency on `homer-core`. Pure graph extraction engine. Could be used independently.
- `homer-core`: Depends on `homer-graphs` for graph extraction. Contains all pipeline logic, store, analyzers, renderers.
- `homer-cli`: Depends on `homer-core`. Thin CLI layer using `clap`.
- `homer-mcp`: Depends on `homer-core`. MCP server using `rmcp` (Anthropic's Rust MCP SDK).

---

## Data Flow: Full Pipeline

### Initial Run (`homer init`)

```
1. Extract
   ├── git.rs: Walk ALL commits → store commit nodes + modification hyperedges
   ├── github.rs: Fetch ALL PRs + issues → store PR/issue nodes + link hyperedges  
   ├── structure.rs: Snapshot file tree → store file nodes + directory hierarchy
   ├── graph.rs: For each source file at HEAD:
   │   ├── Precise (if language has stack graph rules):
   │   │   tree-sitter parse → TSG rules → scope graph → store
   │   │   (also: extract adjacent doc comments → store as node metadata)
   │   ├── Heuristic (fallback):
   │   │   tree-sitter parse → AST walk → extract definitions + calls → store
   │   │   (also: extract adjacent doc comments → store as node metadata)
   │   └── Manifest: parse Cargo.toml/package.json/etc. → external deps → store
   ├── document.rs: For each documentation file (README, ADR, CONTRIBUTING, etc.):
   │   Parse Markdown/RST → store Document nodes → resolve cross-references → store Documents edges
   └── prompt.rs: For each detected agent interaction source:
       ├── Agent rules (CLAUDE.md, .cursor/rules): always extracted → store AgentRule nodes
       └── Session logs (.claude/, .beads/): if opted in → store Prompt/AgentSession nodes

2. Analyze
   ├── behavioral.rs: 
   │   Read commit hyperedges → compute per-file metrics → store
   │   Detect co-change sets → store as hyperedges
   │   Compute documentation coverage + freshness metrics
   │   Compute prompt hotspots + correction hotspots
   ├── centrality.rs:
   │   Load call/import graphs from store → build petgraph
   │   Compute PageRank, betweenness, HITS → store scores per node
   │   Run community detection → store cluster assignments
   ├── temporal.rs:
   │   Load graph snapshots at tagged releases
   │   Compute diffs: new edges, removed edges, centrality deltas
   │   Detect architectural drift → store trend data
   ├── convention.rs:
   │   Scan ASTs for naming patterns, testing patterns, error handling
   │   Validate agent rules against actual code patterns (drift detection)
   │   Compute modal patterns → store as convention records
   ├── task_pattern.rs:
   │   Extract recurring task shapes from prompt history
   │   Build domain vocabulary mappings (human terms → code identifiers)
   │   Identify correction hotspots → store
   └── semantic.rs (gated by salience threshold):
       Filter nodes where composite_salience > threshold
       For each: check for doc comment → if good, skip/simplify LLM call
       For each needing LLM: generate summary → store with content hash for cache

3. Render (per selected renderer)
   Read from store → produce output files → write to repo or output directory
   Handle input/output circularity for AGENTS.md (diff/merge modes)
```

### Incremental Run (`homer update`)

```
1. Extract (only new data)
   ├── git.rs: Walk commits since last checkpoint SHA → store new
   ├── github.rs: Fetch PRs/issues since last number → store new
   ├── structure.rs: Re-snapshot file tree, diff against stored → update changed
   ├── graph.rs: For each changed source file:
   │   Rebuild scope graph → diff against stored graph → store changes
   │   Re-extract doc comments for changed functions/types
   │   Record edge additions/removals for temporal analysis
   ├── document.rs: Re-hash documents → only re-process changed ones
   │   Re-resolve cross-references when documents or referenced code changed
   └── prompt.rs: Process new sessions since last timestamp
       Re-check agent rule files for content hash changes
       Track git history of agent rule files for evolution signal

2. Analyze (only affected computations)
   ├── behavioral.rs: Recompute metrics for files touched in new commits
   │   Update documentation coverage for changed files
   │   Update prompt hotspots with new interaction data
   ├── centrality.rs: 
   │   IF graph topology changed: recompute global metrics (unavoidable)
   │   IF only weights changed: incremental update
   ├── temporal.rs: Extend trend data with new data point
   ├── convention.rs: Recheck conventions for changed files
   │   Re-validate agent rules against updated patterns
   ├── task_pattern.rs: Update task patterns with new prompt data
   └── semantic.rs: Re-summarize only nodes whose:
       - Source code changed, OR
       - Doc comment changed, OR
       - Immediate graph neighborhood changed, OR
       - Composite salience crossed threshold

3. Render (all enabled renderers re-run)
   Renderers always read current state from store — no incremental rendering
   (Rendering is fast relative to analysis)
```

---

## Error Handling Strategy

Homer uses a layered error approach:

```rust
/// Top-level Homer error
#[derive(thiserror::Error, Debug)]
pub enum HomerError {
    #[error("Store error: {0}")]
    Store(#[from] StoreError),
    
    #[error("Extraction error: {0}")]
    Extract(#[from] ExtractError),
    
    #[error("Analysis error: {0}")]
    Analyze(#[from] AnalyzeError),
    
    #[error("Render error: {0}")]
    Render(#[from] RenderError),
    
    #[error("Graph engine error: {0}")]
    Graph(#[from] GraphError),
    
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
}
```

**Principle**: Extraction and analysis errors on individual files should NOT abort the entire pipeline. Homer should process what it can, report what it couldn't, and produce partial results. A parse error in one file shouldn't prevent analysis of the other 10,000 files.

```rust
/// Result of processing a batch of items
pub struct BatchResult<T> {
    pub successes: Vec<T>,
    pub failures: Vec<(PathBuf, HomerError)>,
}
```

---

## Configuration

See [CLI.md](CLI.md) for full configuration specification. The core configuration lives in `.homer/config.toml`:

```toml
[homer]
version = "0.1.0"

[analysis]
depth = "standard"                # shallow | standard | deep | full
llm_salience_threshold = 0.7
max_llm_batch_size = 50
llm_provider = "anthropic"        # anthropic | openai | custom

[extraction]
github_token_env = "GITHUB_TOKEN"
max_pr_history = 500
max_issue_history = 1000
include_patterns = ["**/*.rs", "**/*.py", "**/*.ts"]
exclude_patterns = ["**/vendor/**", "**/node_modules/**"]

[extraction.documents]
enabled = true
include_doc_comments = true
include_patterns = ["README*", "CONTRIBUTING*", "ARCHITECTURE*", "CHANGELOG*", "docs/**/*.md", "adr/**/*.md", "*.tps"]
exclude_patterns = ["**/node_modules/**", "**/vendor/**"]

[extraction.prompts]
enabled = false                   # Opt-in, not opt-out
sources = ["claude-code", "agent-rules"]
redact_sensitive = true
store_full_text = false
hash_session_ids = true

[graph]
languages = ["rust", "python", "typescript", "go"]
fallback_tier = "heuristic"       # heuristic | manifest | none

[renderers]
enabled = ["agents-md", "module-ctx", "risk-map"]

[renderers.agents-md]
output_path = "AGENTS.md"
max_load_bearing_modules = 20
include_change_patterns = true
circularity_mode = "auto"         # auto | diff | merge | overwrite

[renderers.module-ctx]
per_directory = true
filename = ".context.md"

[renderers.spec]
output_path = "spec/"
format = "topos"
```

---

## Concurrency Model

See [PERFORMANCE.md](PERFORMANCE.md) for detailed performance architecture.

Summary:

| Phase | Strategy | Rationale |
|-------|----------|-----------|
| Git walking | Sequential | libgit2 not threadsafe per repo handle |
| File parsing | Rayon parallel | Each file parse is independent |
| Scope graph build | Rayon parallel | Each file's subgraph is isolated |
| Doc comment extraction | Rayon parallel | Piggybacks on file parsing pass |
| Document parsing | Rayon parallel | Each document is independent |
| GitHub API | Tokio async | I/O-bound, concurrent HTTP requests |
| Prompt extraction | Sequential per source | Source-specific parsing, low volume |
| Behavioral analysis | Rayon parallel | Per-file metrics independent |
| Centrality computation | Single-threaded per algo | Algorithms parallelize internally in petgraph |
| Task pattern analysis | Single-threaded | Operates on aggregated prompt data |
| LLM calls | Tokio async | I/O-bound, batched requests |
| Rendering | Rayon parallel | Each renderer is independent |

---

## Security Considerations

- **GitHub tokens**: Read from environment variable, never stored in Homer's database
- **LLM API keys**: Same — environment variable only
- **Database file**: Contains code metadata (function names, file paths, commit messages) but not source code content (source is read on-demand from the git repo)
- **Semantic summaries**: May contain LLM-generated descriptions of code behavior. These are cached locally and never transmitted except to the configured LLM provider
- **Prompt data**: Agent interaction logs may contain sensitive information. Prompt mining is opt-in (`enabled = false` by default). When enabled, `redact_sensitive = true` strips detected API keys, passwords, and personal information. When `store_full_text = false` (default), only structured metadata (file references, task patterns, correction signals) is retained, not raw prompt text
- **Agent rule files**: CLAUDE.md, .cursor/rules, and similar files are committed to the repo and treated as public. These are always extracted when detected, regardless of the prompt mining opt-in flag. In private repos, these files may contain internal conventions, team names, or architectural details that are not intended for external exposure — the `redact_paths` option (default: false) can strip absolute paths and organization-specific identifiers from rendered outputs
- **MCP server**: Homer's MCP server exposes read-only query tools over the analysis database. Security posture:
  - **Binding**: Localhost only by default. Network exposure requires explicit `--mcp-bind` flag
  - **Input validation**: All tool inputs are validated against strict schemas before query execution. No raw SQL passthrough. Parameter types and ranges are enforced (e.g., `limit` capped at 1000, path parameters must be relative, no path traversal)
  - **Tool schema integrity**: Tool descriptors are defined in code (not loaded from external sources) and versioned with the Homer binary. This eliminates tool descriptor poisoning vectors documented in MCP security research
  - **Rate limiting**: Configurable per-tool rate limits (default: 100 requests/minute per tool). Prevents resource exhaustion from runaway agent loops
  - **Audit logging**: All MCP tool invocations are logged to `homer-mcp.log` with timestamp, tool name, parameters, and response size. Logging level configurable (`off`, `summary`, `full`)
  - **Output scope**: MCP tools return analysis results (summaries, scores, relationships), not raw source code. Source snippets included in semantic summaries are bounded (max 10 lines per entity) and controllable via `--mcp-max-snippet-lines`

---

## Testing Strategy

| Level | Scope | Tools |
|-------|-------|-------|
| Unit | Individual functions, data structures | `#[test]`, proptest for property-based |
| Integration | Pipeline stages end-to-end | homer-test crate with fixture repos |
| Benchmark | Performance regression detection | Criterion.rs |
| Fixture repos | Small, controlled git repos with known properties | Created programmatically in tests |

Fixture repos should include:

- **Minimal**: 5 files, 10 commits, known call graph — verifies basic pipeline
- **Multi-language**: Python + TypeScript + Rust — verifies language tier handling
- **Large synthetic**: Generated repo with 1000+ files — verifies performance
- **Known patterns**: Repo with planted co-change patterns, known centrality structure — verifies analysis accuracy
- **Documented**: Repo with README, ADRs, doc comments of varying quality — verifies document extraction and doc-comment-aware analysis
- **Agent-interactive**: Repo with `.claude/` session logs, CLAUDE.md, and `.cursor/rules` — verifies prompt extraction pipeline
