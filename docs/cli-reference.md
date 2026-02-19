# CLI Reference

Complete reference for all 10 Homer commands.

## Global Options

All commands accept:

| Flag | Description |
|------|-------------|
| `-v`, `-vv`, `-vvv` | Increase verbosity (info, debug, trace) |
| `-q` | Quiet mode (errors only) |
| `--help` | Show help |
| `--version` | Show version |

You can also set `RUST_LOG` for fine-grained control: `RUST_LOG=homer_core=debug homer update`.

---

## `homer init`

First-time full analysis of a repository.

```
homer init [OPTIONS] [PATH]
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--depth` | string | `standard` | Analysis depth: `shallow`, `standard`, `deep`, `full` |
| `--languages` | string | — | Comma-separated language list (e.g., `rust,python`) |
| `--no-github` | flag | — | Skip GitHub API extraction |
| `--no-llm` | flag | — | Skip LLM-powered analysis |
| `--db-path` | path | — | Custom database location |
| `--config` | path | `.homer/config.toml` | Config file path |

### Examples

```bash
# Initialize with defaults
homer init

# Fast analysis of a large repo
homer init --depth shallow /path/to/monorepo

# Only Rust and Python, no GitHub
homer init --languages rust,python --no-github

# Custom database location
homer init --db-path /shared/homer-data/project.db
```

### Notes

- Fails if `.homer/` already exists — use `homer update` to refresh an existing database
- Creates `.homer/config.toml` and `.homer/homer.db`
- Database path priority: `--db-path` > `HOMER_DB_PATH` env > `.homer/homer.db`
- Exit code 0 on success; non-zero on failure

---

## `homer update`

Incremental update after new commits.

```
homer update [OPTIONS] [PATH]
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--force` | flag | — | Full re-extraction (clears all checkpoints) |
| `--force-analysis` | flag | — | Recompute all analysis, keep extraction |
| `--force-semantic` | flag | — | Refresh only LLM-derived semantic analyses |

### Examples

```bash
# Incremental update (only new commits)
homer update

# Full re-extraction from scratch
homer update --force

# Keep extracted data, recompute all metrics
homer update --force-analysis

# Only refresh LLM summaries (after model upgrade, for example)
homer update --force-semantic
```

### Notes

- The git extractor processes only commits since the last checkpoint
- Structure and graph extractors re-scan all files but skip unchanged ones via content hashing
- `--force-semantic` clears SemanticSummary, DesignRationale, and InvariantDescription results
- Exits with code 10 if pipeline completed with non-fatal errors

---

## `homer status`

Show database stats, checkpoints, and artifact status.

```
homer status [PATH]
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Examples

```bash
homer status
```

### Output

Displays:
- Database path and size (formatted as B/KB/MB)
- Node counts by kind (sorted descending)
- Edge counts by kind (sorted descending)
- Total analysis results
- Checkpoints (`git_last_sha`, `graph_last_sha` — first 12 chars)
- Pending commits since last checkpoint
- Whether `AGENTS.md` exists and its file size

---

## `homer query`

Query metrics for a file, function, or module.

```
homer query [OPTIONS] <ENTITY>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `ENTITY` | File path, function name, or qualified name to query |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--path` | path | `.` | Path to git repository |
| `--format` | string | `text` | Output format: `text`, `json`, `markdown` (or `md`) |
| `--include` | string | `all` | Comma-separated sections: `summary`, `metrics`, `callers`, `callees`, `history`, `all` |
| `--depth` | integer | `1` | Graph traversal depth for callers/callees (BFS) |

### Examples

```bash
# Query a file
homer query src/store/sqlite.rs

# Query a function with call graph depth 2
homer query "validate_token" --depth 2

# JSON output with specific sections
homer query src/main.rs --format json --include metrics,callers

# Markdown output for documentation
homer query src/auth/ --format markdown
```

### Metrics Shown

- Composite salience (score and classification)
- PageRank (score and rank)
- HITS (hub and authority scores)
- Change frequency (total, 30/90/365-day windows)
- Contributor concentration (bus factor)
- Stability classification
- Community assignment
- Callers and callees (with BFS at `--depth`)
- Recent modification history (up to 20 commits)

---

## `homer graph`

Explore graph analysis: rankings, communities, and visualizations.

```
homer graph [OPTIONS]
```

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--path` | path | `.` | Path to git repository |
| `--type` | string | `call` | Graph type: `call`, `import`, `combined` |
| `--metric` | string | `salience` | Metric: `pagerank`, `betweenness`, `hits`, `salience` |
| `--top` | integer | `20` | Number of top entities to show |
| `--list-communities` | flag | — | List all detected communities |
| `--community` | integer | — | Show members of a specific community ID |
| `--format` | string | `text` | Output format: `text`, `json`, `dot`, `mermaid` |

### Examples

```bash
# Top 20 files by composite salience
homer graph

# Top 10 by PageRank
homer graph --metric pagerank --top 10

# Bridge nodes (betweenness centrality)
homer graph --metric betweenness --top 10

# Export as Graphviz DOT
homer graph --metric salience --format dot > salience.dot
dot -Tsvg salience.dot -o salience.svg

# Export as Mermaid diagram
homer graph --metric pagerank --format mermaid

# List all communities
homer graph --list-communities

# View members of community 3
homer graph --community 3
```

### Notes

- `--community` takes precedence over `--list-communities`, which takes precedence over metric ranking
- Long entity names (>50 chars) are truncated in text output
- `dot` format produces a Graphviz digraph; `mermaid` produces a `graph LR`

---

## `homer diff`

Compare architectural state between two git refs.

```
homer diff [OPTIONS] <REF1> <REF2>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `REF1` | Start reference (tag, branch, or SHA) |
| `REF2` | End reference (tag, branch, SHA, or HEAD) |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--path` | path | `.` | Path to git repository |
| `--format` | string | `text` | Output format: `text`, `json`, `markdown` |
| `--include` | string | all | Comma-separated sections: `topology`, `centrality`, `communities`, `coupling` |

### Examples

```bash
# What changed architecturally in the last 10 commits?
homer diff HEAD~10 HEAD

# Compare two releases
homer diff v1.0 v2.0

# Markdown output for PR descriptions
homer diff main feature-branch --format markdown

# Only topology and centrality
homer diff v1.0 v2.0 --include topology,centrality
```

### Sections

- **topology** — File counts (added/modified/deleted/renamed), changed file list
- **centrality** — High-salience files touched (salience > 0.3)
- **coupling** — Low bus factor files (bus_factor <= 1), affected modules
- **communities** — Community labels affected by the changes

---

## `homer render`

Run specific renderers to regenerate artifacts.

```
homer render [OPTIONS] [PATH]
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--format` | string | — | Comma-separated renderer names to run |
| `--all` | flag | — | Run all 6 renderers |
| `--exclude` | string | — | Comma-separated renderers to exclude (used with `--all`) |
| `--output-dir` | path | repo root | Output directory for artifacts |
| `--dry-run` | flag | — | Show what would be generated without writing |
| `--diff` | flag | — | Show diff between existing artifacts and new output |
| `--merge` / `--no-merge` | bool | `true` | Merge with `<!-- homer:preserve -->` blocks (default: merge) |

### Renderer Names

`agents-md`, `module-ctx`, `risk-map`, `skills`, `topos-spec`, `report`

### Examples

```bash
# Re-render just AGENTS.md
homer render --format agents-md

# Render all artifacts
homer render --all

# All except module context
homer render --all --exclude module-ctx

# Preview what would change
homer render --all --dry-run

# Show diff against existing files
homer render --format agents-md --diff

# Overwrite everything (ignore preserve markers)
homer render --all --no-merge
```

### Notes

- Renderer priority: `--all` > `--format` > config `renderers.enabled`
- `--merge` is enabled by default — `<!-- homer:preserve -->` blocks in existing files are kept
- `--diff` shows the merge result compared to the existing file

---

## `homer snapshot`

Create, list, or delete graph snapshots.

```
homer snapshot [PATH] <COMMAND>
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Subcommands

#### `homer snapshot create <LABEL>`

Create a named snapshot of the current graph state.

```bash
homer snapshot create v1.0-baseline
homer snapshot create pre-refactor
```

#### `homer snapshot list`

List all snapshots with their ID, label, creation time, and node/edge counts.

```bash
homer snapshot list
```

Output columns: ID, LABEL, CREATED, NODES, EDGES

#### `homer snapshot delete <LABEL>`

Delete a snapshot by label.

```bash
homer snapshot delete pre-refactor
```

### Notes

- Auto snapshots are created by the pipeline based on `[graph.snapshots]` config
- Manual snapshots created here are in addition to auto snapshots
- Snapshots enable `homer diff` comparisons and temporal analysis

---

## `homer risk-check`

CI gate: fail if any file exceeds a risk threshold.

```
homer risk-check [OPTIONS] [PATH]
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `PATH` | `.` | Path to git repository |

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--threshold` | float | `0.7` | Risk score threshold (0.0–1.0); fail if any file exceeds this |
| `--filter` | string | — | Only check files whose name contains this pattern |
| `--format` | string | `text` | Output format: `text` or `json` |

### Risk Score Formula

The risk score (0.0–1.0, capped) combines:
- Salience × 0.4
- Bus factor: +0.30 if ≤ 1, +0.15 if ≤ 2
- Change frequency: +0.30 if > 20 changes, +0.20 if > 10, +0.10 if > 5

### Examples

```bash
# Default threshold (0.7)
homer risk-check

# Stricter threshold
homer risk-check --threshold 0.5

# Only check auth files
homer risk-check --filter src/auth/

# JSON output for CI parsing
homer risk-check --format json --threshold 0.6
```

### Exit Codes

- **0** — All files below threshold
- **Non-zero** — One or more files exceed threshold (designed for CI gating)

---

## `homer serve`

Start the MCP server for AI agent integration.

```
homer serve [OPTIONS]
```

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--path` | path | `.` | Path to git repository |
| `--transport` | string | from config | Transport type: `stdio` or `sse` |
| `--host` | string | from config | Host for SSE transport |
| `--port` | integer | from config | Port for SSE transport |

### Examples

```bash
# Start on stdio (for MCP client integration)
homer serve

# Specify the repo path
homer serve --path /path/to/project
```

### Notes

- Currently only `stdio` transport is implemented
- Falls back to `[mcp]` config section, then defaults (`stdio`, `127.0.0.1:3000`)
- See [MCP Integration](mcp-integration.md) for setup guides

---

## Next Steps

- [Getting Started](getting-started.md) — First run walkthrough
- [Configuration](configuration.md) — Full config reference
- [MCP Integration](mcp-integration.md) — AI tool integration
- [Cookbook](cookbook.md) — Common workflow recipes
