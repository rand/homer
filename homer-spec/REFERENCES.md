# References

> Academic citations, prior art, related work, and technical dependencies.

**Related docs:** [GRAPH_ENGINE.md](GRAPH_ENGINE.md) · [ARCHITECTURE.md](ARCHITECTURE.md) · [INTEGRATIONS.md](INTEGRATIONS.md)

---

## Prior Art and Landscape

### Behavioral Code Analysis

**Adam Tornhill / CodeScene**
- Tornhill, A. *Your Code as a Crime Scene* (2015). Pragmatic Bookshelf.
- Tornhill, A. *Software Design X-Rays* (2018). Pragmatic Bookshelf.
- [CodeScene](https://codescene.com/) — commercial behavioral code analysis platform.
- **Relationship to Homer:** CodeScene pioneered mining git history for behavioral insights (hotspots, change coupling, team patterns). Homer adopts these techniques but extends them with graph-theoretic analysis and produces agent-consumable artifacts rather than human dashboards.

**code-maat**
- [code-maat](https://github.com/adamtornhill/code-maat) — Tornhill's open-source tool for mining git history. Python-based, produces CSV reports.
- **Relationship to Homer:** Homer's behavioral analyzer implements similar metrics (change frequency, co-change, churn) but integrates them with graph centrality for composite salience scores.

### Repository Mining Tools

**PyDriller**
- Spadini, D., Aniche, M., & Bacchelli, A. (2018). *PyDriller: Python framework for mining software repositories.* ESEC/FSE 2018.
- [PyDriller](https://github.com/ishepard/pydriller) — Python framework for mining git repositories.
- **Relationship to Homer:** PyDriller provides raw extraction primitives. Homer builds an integrated pipeline with analysis and rendering on top.

**Hercules**
- [Hercules](https://github.com/src-d/hercules) — Git history analysis tool by source{d}. Written in Go.
- **Relationship to Homer:** Hercules focuses on contributor analysis and code ownership. Homer incorporates these as one signal among many.

### Code Navigation and Graph Extraction

**Stack Graphs**
- Creager, D.A. & van Antwerpen, H. (2022). *Stack Graphs: Name Resolution at Scale.* [arXiv:2211.01224](https://arxiv.org/pdf/2211.01224).
- [github/stack-graphs](https://github.com/github/stack-graphs) — Rust implementation (**archived September 2025**).
- [tree-sitter-stack-graphs](https://crates.io/crates/tree-sitter-stack-graphs) — Crate for building stack graphs from tree-sitter grammars. Version 0.10.0.
- [Introducing stack graphs (blog post)](https://github.blog/open-source/introducing-stack-graphs/)
- [Strange Loop 2021 talk (video)](https://www.youtube.com/watch?v=l2R1PTGcwrE)
- **Relationship to Homer:** Homer's `homer-graphs` crate evolves the stack graph formalism — forking the core algorithms, adding language support beyond the archived set (Rust, Go), and extending with call graph projection and temporal diffing. See [GRAPH_ENGINE.md](GRAPH_ENGINE.md).
- **Archival implications:** The `github/stack-graphs` repo was archived September 9, 2025. Published crates remain usable. Homer forks the core logic and TSG language rules rather than depending on the archived repo, ensuring independent evolution.

**Scope Graphs**
- van Antwerpen, H., Néron, P., Tolmach, A., Visser, E., & Wachsmuth, G. (2016). *A constraint language for static semantic analysis based on scope graphs.* PEPM '16.
- Zwaan, A. & van Antwerpen, H. (2023). *Scope graphs: The story so far.* EVCS 2023.
- [Scope Graphs Project (TU Delft)](https://pl.ewi.tudelft.nl/research/projects/scope-graphs/)
- **Relationship to Homer:** Scope graphs are the theoretical foundation of stack graphs. They provide the name binding formalism that Homer uses for precise code navigation.

**Tree-sitter**
- [tree-sitter](https://tree-sitter.github.io/) — Incremental parsing system for programming tools.
- [tree-sitter-graph](https://github.com/tree-sitter/tree-sitter-graph) — DSL for constructing arbitrary graph structures from parsed source code. **Actively maintained** (unlike stack-graphs).
- **Relationship to Homer:** Tree-sitter is Homer's parsing foundation. All code extraction (both precise and heuristic tiers) starts with tree-sitter. The tree-sitter-graph DSL is used for defining scope graph construction rules per-language.

### Repository Representation for AI Agents

**RPG-Encoder**
- Luo, J., Yin, C., et al. (2026). *Closing the Loop: Universal Repository Representation with RPG-Encoder.* [arXiv:2602.02084](https://arxiv.org/abs/2602.02084).
- **Relationship to Homer:** RPG-Encoder proposes a dual-view (semantic + structural) repository representation for agent navigation. Homer shares the insight that both views are necessary but differs fundamentally:

  | Dimension | RPG-Encoder | Homer |
  |-----------|-------------|-------|
  | Temporal | Snapshot only | Full history + evolution tracking |
  | Graph analysis | None (no centrality, no community detection) | PageRank, betweenness, HITS, Louvain |
  | Construction | LLM-heavy (every node summarized) | Algorithmic-first, LLM salience-gated |
  | Output | Opaque agent-queryable graph | Durable human-readable artifacts |
  | PR/issue mining | None | Core capability |
  | Incremental cost | 95.7% reduction claimed | Similar target via memoization + freshness |

  **Concepts adopted from RPG-Encoder:**
  - Dual-view (semantic + structural) as a foundational design principle
  - Hierarchical construction: recovering latent architecture by clustering features into `area/category/subcategory`
  - Incremental evolution via commit-diff processing with intent-shift detection
  - Three agentic tool patterns (search, fetch, explore) → Homer's MCP tools
  - Artifact grounding via LCA (lowest common ancestor) to map abstract clusters to directory scopes

**GitHub Spec Kit**
- [GitHub Spec Kit](https://github.com/features/spec) — Spec-driven development starting from specs → code.
- **Relationship to Homer:** Spec Kit goes forward (spec → code). Homer goes reverse (code → spec). They're complementary — Homer bootstraps a spec that Spec Kit or Topos then maintains.

**Augment Code Context Lineage**
- [Augment Code](https://augmentcode.com/) — IDE integration summarizing commits with LLMs.
- **Relationship to Homer:** Augment summarizes individual commits for IDE context. Homer builds a comprehensive, persistent knowledge base with cross-commit analysis.

### AGENTS.md / CLAUDE.md Generators

**Claude Code `/init`**
- Claude Code's `/init` command generates a CLAUDE.md from current repo state.
- **Relationship to Homer:** `/init` produces a point-in-time snapshot. Homer produces a history-informed, graph-analyzed document with insights that snapshot-based tools cannot generate (change patterns, danger zones, architectural evolution, graph salience).

---

## Graph Algorithm Theory

### Centrality Metrics

**PageRank**
- Brin, S. & Page, L. (1998). *The anatomy of a large-scale hypertextual Web search engine.* Computer Networks, 30(1-7), pp. 107-117.
- **Usage in Homer:** Applied to call graphs to identify load-bearing code entities — functions that many other functions depend on transitively. See [ANALYZERS.md#centrality-analyzer](ANALYZERS.md#centrality-analyzer).

**Betweenness Centrality**
- Brandes, U. (2001). *A faster algorithm for betweenness centrality.* Journal of Mathematical Sociology, 25(2), pp. 163-177.
- **Usage in Homer:** Applied to import graphs to identify bridge modules — modules that connect otherwise-separate subsystems. High betweenness = architectural bottleneck.

**HITS (Hyperlink-Induced Topic Search)**
- Kleinberg, J.M. (1999). *Authoritative sources in a hyperlinked environment.* Journal of the ACM, 46(5), pp. 604-632.
- **Usage in Homer:** Distinguishes orchestrator functions (hubs: call many things) from utility functions (authorities: called by many things). This distinction is important for agents — orchestrators need architectural understanding, utilities need interface stability guarantees.

### Community Detection

**Louvain Method**
- Blondel, V.D., Guillaume, J.-L., Lambiotte, R., & Lefebvre, E. (2008). *Fast unfolding of communities in large networks.* Journal of Statistical Mechanics, 2008(10).
- **Usage in Homer:** Discovers natural module boundaries in the dependency graph that may diverge from directory structure. When detected communities don't align with directories, Homer flags this as an architectural insight. See [ANALYZERS.md#community-detection](ANALYZERS.md#community-detection).

---

## Rust Ecosystem Dependencies

### Core Libraries

| Crate | Version | Purpose | License |
|-------|---------|---------|---------|
| `gix` | latest | Pure-Rust git implementation (gitoxide) | MIT/Apache-2.0 |
| `rusqlite` | latest | SQLite bindings with bundled SQLite | MIT |
| `tree-sitter` | 0.25+ | Incremental parsing framework | MIT |
| `tree-sitter-graph` | 0.7+ | Graph construction DSL for tree-sitter | MIT/Apache-2.0 |
| `petgraph` | latest | In-memory graph data structures and algorithms | MIT/Apache-2.0 |
| `rayon` | latest | Data parallelism (CPU-bound work) | MIT/Apache-2.0 |
| `tokio` | latest | Async runtime (I/O-bound work) | MIT |
| `reqwest` | latest | HTTP client (GitHub API, LLM APIs) | MIT/Apache-2.0 |
| `serde` | latest | Serialization framework | MIT/Apache-2.0 |
| `serde_json` | latest | JSON serialization | MIT/Apache-2.0 |
| `toml` | latest | TOML config parsing | MIT/Apache-2.0 |
| `clap` | latest | CLI framework with derive macros | MIT/Apache-2.0 |
| `rmcp` | latest | Model Context Protocol SDK for Rust | MIT |
| `thiserror` | latest | Derive macro for error types | MIT/Apache-2.0 |
| `anyhow` | latest | Application-level error handling | MIT/Apache-2.0 |
| `chrono` | latest | Date/time handling | MIT/Apache-2.0 |
| `tracing` | latest | Structured logging and diagnostics | MIT |
| `indicatif` | latest | Progress bars for CLI | MIT |

### Tree-sitter Grammars

| Grammar | Language | Resolution Tier | Source |
|---------|----------|----------------|--------|
| `tree-sitter-python` | Python | Precise (stack graph rules exist) | tree-sitter org |
| `tree-sitter-typescript` | TypeScript | Precise (stack graph rules exist) | tree-sitter org |
| `tree-sitter-javascript` | JavaScript | Precise (stack graph rules exist) | tree-sitter org |
| `tree-sitter-java` | Java | Precise (stack graph rules exist) | tree-sitter org |
| `tree-sitter-rust` | Rust | Heuristic (stack graph rules planned) | tree-sitter org |
| `tree-sitter-go` | Go | Heuristic (stack graph rules planned) | tree-sitter org |
| `tree-sitter-c` | C | Heuristic | tree-sitter org |
| `tree-sitter-cpp` | C++ | Heuristic | tree-sitter org |
| `tree-sitter-ruby` | Ruby | Heuristic | tree-sitter org |

### Benchmark and Test

| Crate | Purpose |
|-------|---------|
| `criterion` | Statistical benchmark framework |
| `proptest` | Property-based testing |
| `insta` | Snapshot testing for complex outputs |
| `tempfile` | Temporary directories for integration tests |
| `assert_cmd` | CLI integration testing |
| `wiremock` | HTTP mock server for GitHub API tests |

---

## Storage References

**SQLite**
- [SQLite](https://sqlite.org/) — Self-contained, serverless SQL database.
- Homer uses SQLite via `rusqlite` with WAL mode for concurrent reads during analysis.
- See [STORE.md](STORE.md) for the hypergraph schema design.

**libSQL / Turso** (potential future migration)
- [libSQL](https://github.com/tursodatabase/libsql) — Fork of SQLite with open contributions, embedded replicas, native vector search.
- [Turso](https://github.com/tursodatabase/turso) — Full SQLite rewrite in Rust with MVCC, CDC, full-text search via tantivy.
- **Relationship to Homer:** Homer's `HomerStore` trait abstracts the storage backend. Current implementation uses `rusqlite`. libSQL/Turso becomes relevant if Homer adds team-shared knowledge bases (replication) or semantic similarity search (vector search). See [EVOLUTION.md](EVOLUTION.md).

**Hypergraph Data Model**
- Berge, C. (1984). *Hypergraphs: Combinatorics of Finite Sets.* North-Holland.
- **Usage in Homer:** Homer uses hyperedges to represent n-ary relationships (commits touching multiple files, co-change sets, semantic clusters). See [STORE.md#hypergraph-data-model](STORE.md#hypergraph-data-model).

**Influenced by: Loop's Hypergraph Memory**
- [rand/loop](https://github.com/rand/loop) — RLM orchestration monorepo with SQLite-backed hypergraph knowledge store and tiered lifecycle.
- **Relationship to Homer:** Homer's hypergraph storage design is influenced by Loop's approach to tiered lifecycle management and n-ary relationship modeling.

---

## Related Projects (By the Same Author)

| Project | Description | Homer Relationship | Reference |
|---------|-------------|-------------------|-----------|
| [Topos](https://github.com/rand/topos) | Semantic contract language for human-AI collaboration | Homer's `spec` renderer emits `.tps` files; Homer enriches Topos drift detection | [INTEGRATIONS.md#topos](INTEGRATIONS.md#topos) |
| [Ananke](https://github.com/rand/ananke) | Constraint-driven code generation | Homer could feed constraint extraction via stability/invariant data | [INTEGRATIONS.md#ananke](INTEGRATIONS.md#ananke) |
| [Loop](https://github.com/rand/loop) | RLM orchestration with hypergraph memory | Influenced Homer's storage design; potential shared memory patterns | [INTEGRATIONS.md#loop](INTEGRATIONS.md#loop) |

---

## Key Research Concepts Used

| Concept | Source | Homer Application |
|---------|--------|-------------------|
| Behavioral code analysis | Tornhill (2015, 2018) | Change frequency, co-change, churn metrics |
| Scope graphs / stack graphs | van Antwerpen et al. (2016), Creager & van Antwerpen (2022) | Precise name resolution for call graph construction |
| PageRank centrality | Brin & Page (1998) | Identifying load-bearing code entities |
| Betweenness centrality | Brandes (2001) | Identifying bridge/bottleneck modules |
| Hub/authority analysis | Kleinberg (1999) | Distinguishing orchestrators from utilities |
| Community detection | Blondel et al. (2008) | Discovering natural module boundaries |
| Dual-view representation | RPG-Encoder (2026) | Combining semantic and structural views |
| Hypergraph modeling | Berge (1984) | N-ary relationships (co-change sets, commit → files) |
| Incremental computation | Salsa framework | Memoized recomputation on change |
| Documentation quality analysis | Aghajani et al. (2019) | Documentation coverage, freshness, and quality metrics |
| Developer behavior mining | Vasilescu et al. (2015) | Mining AI interaction signals as software engineering data |

---

## Documentation Analysis

**Aghajani, E., et al. (2019).** *Software documentation issues unveiled.* ICSE 2019.
- Taxonomy of documentation problems in software projects — informs Homer's documentation quality metrics (coverage, freshness, staleness detection).
- **Relationship to Homer:** Homer's document extractor and documentation coverage/freshness metrics in the behavioral analyzer are informed by this taxonomy of documentation problems.

---

## Agent Interaction Mining

No direct academic precedent exists for mining AI coding agent interactions as a software engineering signal. This is novel territory for Homer.

**Related work:**
- Vasilescu, B., et al. (2015). *Quality and productivity outcomes relating to continuous integration in GitHub.* ESEC/FSE 2015. (Mining developer behavior from CI signals — analogous approach applied to AI interaction signals.)
- **Relationship to Homer:** Homer's prompt extractor applies the same principle (mining automated interaction logs for software engineering insights) to a new category of signals: AI agent interactions, corrections, task patterns, and domain vocabulary.

---

## Technology Evaluations

**facet.rs**
- [facet.rs](https://facet.rs) — Rust reflection library. Version 0.42+ (as of early 2026), iterating rapidly (~10 minor versions/month). MIT/Apache-2.0.
- [Introducing facet: Reflection for Rust](https://fasterthanli.me/articles/introducing-facet-reflection-for-rust) — Amos Wenger, June 2025.
- **Status for Homer:** Deferred to post-v1. Monitor stability. Highest-value integration points are structural diffing (`facet-diff`) for incremental invalidation and MCP schema generation from Rust doc comments. See [EVOLUTION.md](EVOLUTION.md#facetrs-deferred-to-post-v1) for full evaluation.
