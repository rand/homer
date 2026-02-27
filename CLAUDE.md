# Homer

Repository mining tool for agentic development context.

## Build & Test

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests (unit + integration)
cargo clippy --workspace -- -D warnings  # Lint (must be clean)
cargo fmt --all -- --check       # Format check
```

## Architecture

Cargo workspace with 5 crates:

- **homer-core** — Pipeline, extractors, analyzers, renderers, SQLite store
- **homer-graphs** — Tree-sitter scope graph extraction (13 languages)
- **homer-cli** — `homer` binary (clap subcommands: init, update, status, query, graph, diff, render, snapshot, risk-check, serve)
- **homer-mcp** — MCP server (rmcp, 5 tools)
- **homer-test** — Integration test fixtures and helpers

## Pipeline Flow

Extract → Auto Snapshots → Analyze → Render

1. **Extractors** (git, structure, graph, document, github, gitlab, prompt) populate the hypergraph store
2. **Analyzers** (behavioral, centrality, community, temporal, convention, task pattern, semantic) compute derived metrics in topological order
3. **Renderers** (agents-md, module-ctx, risk-map, skills, topos-spec, report) produce output artifacts

## Key Conventions

- Edition 2024, MSRV 1.85, `unsafe` forbidden
- Clippy pedantic enabled (`-D warnings` in CI)
- `gix` for git (not git2), `rusqlite` for SQLite with WAL mode
- `async_trait` for all async traits
- Content hashes stored as `u64` in Rust, cast to `i64` for SQLite
- Pipeline stages return errors without aborting — errors collected in `PipelineResult`
- AGENTS.md supports `<!-- homer:preserve -->` blocks for human content

## Spec

Design documents in `homer-spec/` — source of truth for types, schemas, algorithms.
