# Homer

Repository intelligence for agentic development. Homer mines git repositories — commits, diffs, call graphs, import graphs, documentation, and AI agent interactions — and produces artifacts that accelerate AI-assisted coding: AGENTS.md files, module context maps, risk maps, and graph metrics.

## Why Homer

AI coding agents work better with context. Homer analyzes your codebase and produces structured context that tells agents which files are load-bearing, which areas are fragile, what naming conventions actually are (not what someone wished), and what change patterns look like for common tasks.

The key insight: **salience is not change frequency**. A utility module half the codebase depends on but hasn't been touched in a year is invisible to behavioral analysis alone — but it's arguably the most important thing for an agent to understand deeply, because the blast radius of a bad change is enormous. Homer surfaces these "quiescent high-centrality" nodes through graph analysis.

## Quick Start

```bash
# Build from source (requires Rust 1.85+)
cargo install --path homer-cli

# Initialize Homer on a git repository
cd your-project
homer init

# See what Homer found
homer status

# Query a specific file
homer query src/auth/validate.rs

# View the most important files by composite salience
homer graph --metric salience --top 20

# After new commits, update incrementally
homer update
```

Homer creates a `.homer/` directory containing a SQLite database and configuration. It generates `AGENTS.md` at the project root, per-directory `.context.md` files, and a `homer-risk.json` risk map.

## What Homer Produces

| Artifact | File | Consumer |
|----------|------|----------|
| **AGENTS.md** | `AGENTS.md` | AI coding agents (Claude Code, Cursor, etc.) |
| **Module Context** | `*/.context.md` | AI agents (scoped per directory) |
| **Risk Map** | `homer-risk.json` | AI agents, CI pipelines |
| **Graph Metrics** | In database | `homer query`, `homer graph`, MCP server |

## Commands

| Command | Purpose |
|---------|---------|
| `homer init [path]` | First-time full analysis of a repository |
| `homer update [path]` | Incremental update after new commits |
| `homer status [path]` | Show database stats, checkpoints, artifact status |
| `homer query <entity>` | Query metrics for a file, function, or module |
| `homer graph` | Explore graph analysis (PageRank, betweenness, communities) |
| `homer diff <ref1> <ref2>` | Compare architectural state between two git refs |
| `homer serve` | Start MCP server for AI agent integration |

See [docs/getting-started.md](docs/getting-started.md) for detailed usage.

## How It Works

Homer combines four disciplines:

1. **Behavioral analysis** — Mining git history for change frequency, churn velocity, co-change patterns, contributor concentration (bus factor)
2. **Structural graph analysis** — Call graphs, import graphs, centrality metrics (PageRank, betweenness, HITS), community detection (Louvain)
3. **Composite salience** — Combining behavioral and structural signals into a single score that identifies the most important code, including stable high-centrality nodes that behavioral analysis alone would miss
4. **Tree-sitter extraction** — Heuristic parsing of function definitions, call sites, imports, and doc comments for Rust, Python, TypeScript, JavaScript, Go, and Java

The pipeline runs in three stages: **Extract** (git history, file structure, call/import graphs, documents) -> **Analyze** (behavioral metrics, centrality, community detection, composite salience) -> **Render** (AGENTS.md, context maps, risk map).

See [docs/concepts.md](docs/concepts.md) for a deeper explanation.

## Configuration

Homer stores configuration in `.homer/config.toml`. Key settings:

```toml
[analysis]
depth = "standard"  # shallow | standard | deep | full

[extraction]
max_commits = 2000  # 0 = unlimited

[graph]
languages = "auto"  # or ["rust", "python", "typescript"]

[renderers]
enabled = ["agents-md", "module-ctx", "risk-map"]
```

See [docs/configuration.md](docs/configuration.md) for the full reference.

## Supported Languages

| Language | Extraction Tier |
|----------|----------------|
| Rust | Heuristic (tree-sitter) |
| Python | Heuristic (tree-sitter) |
| TypeScript | Heuristic (tree-sitter) |
| JavaScript | Heuristic (tree-sitter) |
| Go | Heuristic (tree-sitter) |
| Java | Heuristic (tree-sitter) |

## Architecture

Cargo workspace with 5 crates:

- **homer-core** — Pipeline orchestration, extractors, analyzers, renderers, SQLite store
- **homer-graphs** — Tree-sitter heuristic extraction engine (6 languages)
- **homer-cli** — `homer` binary (clap-based CLI)
- **homer-mcp** — MCP server for AI agent integration
- **homer-test** — Integration test fixtures and helpers

See [homer-spec/](homer-spec/) for full design specifications.

## Building from Source

```bash
# Prerequisites: Rust 1.85+
rustup update stable

# Build
cargo build --workspace --release

# Run tests
cargo test --workspace

# The binary is at target/release/homer
```

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/getting-started.md) | Installation, first run, interpreting results |
| [Concepts](docs/concepts.md) | How Homer works — pipeline, data model, algorithms |
| [Configuration](docs/configuration.md) | Full `.homer/config.toml` reference |
| [Troubleshooting](docs/troubleshooting.md) | Common issues and solutions |
| [Specification](homer-spec/README.md) | Full design specification (12 documents) |

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for community standards.

This project is maintained by a queer developer. Everyone who shares our values of kindness and respect is welcome here. Bigotry is not tolerated.

## License

MIT — see [LICENSE](LICENSE).
