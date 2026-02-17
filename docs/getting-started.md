# Getting Started

This guide walks through installing Homer, running it on a repository, and interpreting the results.

## Prerequisites

- **Rust 1.85+** — Homer uses Edition 2024 features
- **Git** — The repository you're analyzing must be a git repo
- A Unix-like OS (Linux or macOS) — Windows is untested

## Installation

Build from source:

```bash
git clone https://github.com/rand/homer.git
cd homer
cargo install --path homer-cli
```

This installs the `homer` binary to `~/.cargo/bin/`.

Verify the installation:

```bash
homer --version
homer --help
```

## First Run

Navigate to a git repository and initialize Homer:

```bash
cd your-project
homer init
```

Homer will:
1. Create a `.homer/` directory with a SQLite database and `config.toml`
2. Walk the git history (up to 2000 commits by default)
3. Scan the file tree and parse source files with tree-sitter
4. Build call and import graphs
5. Run behavioral analysis (change frequency, churn, bus factor, co-change)
6. Run graph analysis (PageRank, betweenness centrality, HITS, community detection)
7. Compute composite salience scores
8. Generate output artifacts

On completion, Homer reports what it found:

```
Homer initialized in /path/to/your-project

  Nodes extracted: 1380
  Edges created:   1963
  Analyses run:    2033
  Artifacts:       4
  Duration:        6.48s

  Database: /path/to/your-project/.homer/homer.db
  Config:   /path/to/your-project/.homer/config.toml
  Output:   /path/to/your-project/AGENTS.md
```

## Generated Artifacts

After `homer init`, you'll find these new files:

### AGENTS.md

A structured context file at the project root, designed for AI coding agents. It contains:

- **Build & Test** — Commands to build, test, and lint the project (extracted from CI config and manifests)
- **Module Map** — Directory structure with per-module descriptions
- **Change Patterns** — Groups of files that frequently change together (co-change sets)
- **Danger Zones** — Files with high change frequency but low bus factor (single-contributor risk)
- **Conventions** — Naming patterns, error handling style, testing conventions

AI tools like Claude Code, Cursor, and Windsurf read this file to understand the project before making changes.

### .context.md files

Per-directory context files providing scoped information about each module. These give AI agents focused context when working in a specific directory.

### homer-risk.json

Machine-readable risk annotations in JSON format. Contains per-file risk factors (high salience + high churn, low bus factor, etc.) that CI pipelines or agent guardrails can consume.

## Exploring Results

### Check status

```bash
homer status
```

Shows database size, node/edge counts by type, checkpoints, and artifact status.

### Query an entity

```bash
# Query a file
homer query src/store/sqlite.rs

# Query a function
homer query "validate_token"

# JSON output
homer query src/main.rs --format json
```

Shows metrics for the entity: composite salience, PageRank rank, HITS scores, change frequency, bus factor, stability classification, community assignment, and graph edges (calls, imports).

### View graph rankings

```bash
# Top 20 files by composite salience (default)
homer graph

# Top 10 by PageRank
homer graph --metric pagerank --top 10

# Top files by betweenness centrality (bridge nodes)
homer graph --metric betweenness --top 10

# Export as DOT for Graphviz
homer graph --metric salience --format dot > salience.dot

# Export as Mermaid diagram
homer graph --metric pagerank --format mermaid

# JSON output
homer graph --metric salience --format json
```

### View communities

Homer detects communities of files that are structurally coupled through import graphs using the Louvain algorithm:

```bash
# List all communities
homer graph --list-communities

# View members of a specific community
homer graph --community 3
```

### Compare git refs

```bash
# What changed architecturally between two commits?
homer diff HEAD~10 HEAD

# Between tags
homer diff v1.0 v2.0

# Markdown output for PR descriptions
homer diff main feature-branch --format markdown
```

Shows topology changes, high-salience files touched, bus factor risks, affected modules, and affected communities.

## Incremental Updates

After new commits, update Homer's database incrementally:

```bash
homer update
```

The git extractor processes only commits since the last checkpoint. Structure and graph extractors use content-hash-based upsert semantics, so unchanged files are skipped efficiently.

Force a full re-extraction if the database seems stale:

```bash
homer update --force
```

Force just re-analysis (keep extracted data, recompute all metrics):

```bash
homer update --force-analysis
```

## MCP Server

Homer exposes its query capabilities as an MCP (Model Context Protocol) server, allowing AI agents to query the knowledge base directly:

```bash
homer serve
```

This starts a JSON-RPC server on stdio. Configure it in your AI tool's MCP settings:

```json
{
  "mcpServers": {
    "homer": {
      "command": "homer",
      "args": ["serve", "--path", "/path/to/your/project"]
    }
  }
}
```

The MCP server provides tools for querying entities, graph metrics, risk levels, co-change patterns, and conventions.

## Analysis Depth

Control how deeply Homer analyzes your repository:

```bash
homer init --depth shallow   # Fast: last 500 commits, no GitHub, no LLM
homer init --depth standard  # Default: last 2000 commits
homer init --depth deep      # Thorough: all commits
homer init --depth full      # Maximum: all commits, all PRs, LLM enrichment
```

| Level | Git History | Graph | Behavioral | Centrality |
|-------|-----------|-------|-----------|-----------|
| `shallow` | Last 500 commits | Heuristic | Yes | Yes |
| `standard` | Last 2000 commits | Heuristic | Yes | Yes |
| `deep` | All commits | Heuristic | Yes | Yes |
| `full` | All commits | Heuristic | Yes | Yes |

## Verbosity

```bash
homer init -v      # Info-level logging
homer init -vv     # Debug-level logging
homer init -vvv    # Trace-level logging
homer init -q      # Quiet: errors only
```

You can also set the `RUST_LOG` environment variable for fine-grained control:

```bash
RUST_LOG=homer_core=debug homer update
```

## What to Commit

Add these to version control:
- `AGENTS.md` — Useful for all contributors and AI agents
- `.context.md` files — Useful for AI agents
- `.homer/config.toml` — Share configuration across the team

Add these to `.gitignore`:
- `.homer/homer.db` — Machine-specific, regenerated by `homer init`
- `homer-risk.json` — Regenerated on each run (optional to commit)

## Next Steps

- [Concepts](concepts.md) — Understand how Homer's pipeline and algorithms work
- [Configuration](configuration.md) — Customize extraction, analysis, and rendering
- [Troubleshooting](troubleshooting.md) — Common issues and solutions
