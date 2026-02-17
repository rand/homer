# Homer: Repository Mining for Agentic Development

> *"Tell me, O Muse, of that ingenious hero who travelled far and wide..."*

**Version**: 0.2.0-spec  
**Status**: Implementation Specification  
**Language**: Rust  
**License**: MIT  

---

## What Homer Is

Homer is a CLI tool and library that **mines git repositories** — commits, diffs, PRs, issues, call graphs, import graphs, documentation, and AI agent interactions — and produces **artifacts that accelerate agentic development**: CLAUDE.md/AGENTS.md files, module context maps, change pattern skills, reverse-engineered specifications, and risk maps.

Homer combines four disciplines that have never been unified:

1. **Behavioral code analysis** (Adam Tornhill / CodeScene): mining git history for hotspots, change coupling, team patterns
2. **Structural graph analysis**: call graphs, import graphs, centrality metrics (PageRank, betweenness, HITS), community detection
3. **LLM-powered semantic extraction**: intent summarization, design rationale extraction, convention detection
4. **Documentation and agent interaction mining**: extracting knowledge from docs, doc comments, AI agent sessions, and curated agent context files

The unique insight: **salience ≠ change frequency**. A utility module half the codebase depends on but untouched for a year is invisible to behavioral analysis but arguably the most important thing for an agent to understand deeply (blast radius if changed is enormous). Homer surfaces these "quiescent high-centrality" nodes through graph analysis, then enriches them with semantic understanding.

## Why "Homer"

Homer tells the story of a project. Like the poet who preserved civilization's knowledge through structured oral tradition, Homer preserves the accumulated wisdom of a codebase — its architecture, its patterns, its decisions, its evolution — in forms that both humans and AI agents can consume.

The output artifacts are *epics*: compressed, structured narratives of how a codebase came to be, what it is, and what it intends.

## Specification Documents

| Document | Description |
|----------|-------------|
| **[ARCHITECTURE.md](ARCHITECTURE.md)** | System architecture, pipeline, data flow, crate structure |
| **[STORE.md](STORE.md)** | Hypergraph data model, SQLite schema, incrementality protocol |
| **[EXTRACTORS.md](EXTRACTORS.md)** | Git history, GitHub API, structure, graph, document, and prompt extraction |
| **[GRAPH_ENGINE.md](GRAPH_ENGINE.md)** | Stack graphs evolution, tree-sitter, language support tiers |
| **[ANALYZERS.md](ANALYZERS.md)** | Behavioral, centrality, temporal, semantic, convention, and task pattern analysis |
| **[RENDERERS.md](RENDERERS.md)** | All output artifact formats and generation logic |
| **[CLI.md](CLI.md)** | Command interface, configuration, MCP server surface |
| **[PERFORMANCE.md](PERFORMANCE.md)** | Parallelism, memory, caching, benchmarking strategy |
| **[EVOLUTION.md](EVOLUTION.md)** | Plugin system, schema versioning, extensibility design, technology evaluations |
| **[INTEGRATIONS.md](INTEGRATIONS.md)** | Topos, Ananke, Loop — parallel project connections |
| **[REFERENCES.md](REFERENCES.md)** | All citations, prior art, research sources |

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust throughout | Performance, tree-sitter native, single binary deployment |
| Storage | SQLite via `rusqlite`, trait-abstracted | Zero deployment complexity, portable, upgradeable to libSQL |
| Graph extraction | Forked stack-graphs + tree-sitter fallback | Precise name resolution where available, breadth elsewhere |
| Graph algorithms | `petgraph` in-memory | Mature Rust library, load from SQLite → compute → write back |
| LLM integration | HTTP API (reqwest + serde) | Provider-agnostic, no Python dependency |
| Incremental computation | Salsa-inspired memoization | Only recompute what changed |
| Parallelism | Rayon (CPU) + Tokio (I/O) | Parse/analyze in parallel, async for network |
| Serialization | serde ecosystem (defer facet.rs to post-v1) | Ecosystem maturity; facet.rs promising but too volatile |
| Doc comments | Metadata on code nodes, not separate nodes | Avoids graph inflation; doc comments are attributes of code |
| Prompt mining | Opt-in with privacy controls | Sensitive data requires explicit consent and redaction |

## Value Proposition

What Homer produces that **no existing tool does**:

1. **Graph-theoretic salience + temporal evolution**: PageRank on call graphs tracked over time, correlated with behavioral signals. Surfaces insights like "module X became 3× more central over 6 months but has zero tests and one contributor."

2. **Hyperedge-native co-change analysis**: Not "files A and B correlate" but "files {A, B, C, D} form a change set that moves together 87% of the time."

3. **Agent-consumable artifacts from deep analysis**: CLAUDE.md that tells agents which functions are load-bearing, which areas are fragile, what naming conventions actually are (not what someone wished), and what change patterns look like for common tasks.

4. **Reverse-engineered specifications**: Derives [Topos](https://github.com/rand/topos)-formatted specs from accumulated history — closing the loop for brownfield codebases that have code but no specification.

5. **Documentation intelligence**: Correlates doc comments with graph centrality — a high-PageRank function with a good doc comment can skip LLM summarization, while a high-PageRank function with *no* doc comment is a risk flag. Detects documentation staleness and coverage gaps.

6. **Agent interaction mining**: Extracts knowledge from AI coding agent sessions (Claude Code, Cursor, Windsurf, Cline) — task patterns, correction hotspots, domain vocabulary mappings, and areas that repeatedly confuse agents. This is a record of the human-codebase interface that exists nowhere else.

## Non-Goals

- Homer is not an IDE plugin (though it exposes MCP tools that IDEs can consume)
- Homer is not a code generation tool (it produces *context* for generators)
- Homer is not a formal verification system (see [Ananke](INTEGRATIONS.md#ananke) for that direction)
- Homer is not a real-time system (it runs on-demand or in CI, not in-editor)
- Homer does not require LLM access for basic functionality (LLM is the enrichment layer, not the core)
- Homer does not store raw source code in its database (source is read on-demand from the git repo)
- Homer does not store raw prompt text by default (privacy-first; only structured metadata is retained unless explicitly opted in)

## Quick Start (Target UX)

```bash
# First-time analysis of a repository
cd my-project
homer init

# Incremental update after new commits
homer update

# Generate artifacts
homer render --format agents-md    # → AGENTS.md
homer render --format module-ctx   # → per-directory context files
homer render --format skills       # → Claude Code skills
homer render --format spec         # → reverse-engineered .tps spec
homer render --format report       # → HTML report with visualizations
homer render --all                 # → everything

# Query the knowledge base
homer query src/auth/validate.rs   # What Homer knows about this file
homer graph --metric pagerank      # Top functions by PageRank
homer diff v1.0..v2.0             # Architectural changes between releases

# Start MCP server for agent integration
homer serve
```
