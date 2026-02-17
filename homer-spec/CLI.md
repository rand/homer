# Homer CLI & MCP Interface

> Command-line interface, configuration, and MCP server specification.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [RENDERERS.md](RENDERERS.md) · [EXTRACTORS.md](EXTRACTORS.md)

---

## CLI Framework

**Crate**: `clap` v4 with derive macros  
**Binary name**: `homer`

---

## Commands

### `homer init [path]`

First-time full extraction and analysis of a repository.

```
homer init [OPTIONS] [PATH]

Arguments:
  [PATH]  Path to git repository (default: current directory)

Options:
  --depth <DEPTH>        Analysis depth: shallow, standard, deep, full [default: standard]
  --no-github            Skip GitHub API extraction
  --no-llm              Skip LLM-powered analysis
  --languages <LANGS>    Comma-separated list of languages to analyze [default: auto-detect]
  --db-path <PATH>       Custom database location [default: .homer/homer.db]
  --config <PATH>        Config file path [default: .homer/config.toml]
  -v, --verbose          Increase verbosity (-v, -vv, -vvv)
  -q, --quiet            Suppress non-error output
```

**Depth levels**:

| Level | Git History | GitHub | Graph Extraction | Behavioral | Centrality | Semantic (LLM) |
|-------|-----------|--------|-----------------|-----------|-----------|----------------|
| `shallow` | Last 500 commits | No | Heuristic only | Yes | Yes | No |
| `standard` | Last 2000 commits | Yes (last 200 PRs) | Precise where available | Yes | Yes | Top 50 entities |
| `deep` | All commits | Yes (last 500 PRs) | Precise where available | Yes | Yes | Top 200 entities |
| `full` | All commits | Yes (all PRs/issues) | Precise where available | Yes | Yes | All high-salience |

### `homer update`

Incremental update — process new data since last run.

```
homer update [OPTIONS]

Options:
  --force               Force full re-extraction (ignore checkpoints)
  --force-analysis      Force re-analysis (keep extraction, recompute all analysis)
  --force-semantic      Force re-run LLM analysis (even if cache is valid)
  -v, --verbose
```

### `homer render`

Generate output artifacts.

```
homer render [OPTIONS]

Options:
  --format <FORMATS>    Comma-separated: agents-md, module-ctx, skills, spec, report, risk-map
  --all                 Generate all enabled renderers
  --exclude <FORMATS>   Exclude specific formats when using --all
  --output-dir <PATH>   Output directory [default: repo root]
  --dry-run            Show what would be generated without writing files
  --diff               For agents-md: show what Homer would add/change vs existing file
  --merge              For agents-md: merge Homer output with human-curated sections
```

### `homer query`

Query the Homer knowledge base.

```
homer query <ENTITY> [OPTIONS]

Arguments:
  <ENTITY>  File path, function name, or qualified name to query

Options:
  --format <FORMAT>     Output format: text, json, markdown [default: text]
  --include <SECTIONS>  What to include: summary, metrics, callers, callees, history, all
  --depth <N>           Graph traversal depth for callers/callees [default: 1]

Examples:
  homer query src/auth/validate.rs
  homer query "AuthService::validate_token"
  homer query src/auth/ --include metrics,summary
```

**Output for `homer query src/auth/validate.rs`**:
```
File: src/auth/validate.rs
Module: src/auth/
Language: Rust (Precise tier)

Metrics:
  Composite Salience: 0.87 (FoundationalStable)
  PageRank: 0.82 (rank #3 of 1,247)
  Betweenness: 0.34
  Change Frequency: 4 commits in last 90 days
  Bus Factor: 2 (contributors: alice, bob)
  Stability: StableCore

Functions:
  validate_token (salience: 0.91)
    Called by: 23 functions
    Calls: 5 functions
    Summary: "Validates JWT tokens against configured signing keys..."
    
  refresh_token (salience: 0.78)
    Called by: 8 functions
    ...

Co-changes with:
  src/auth/middleware.rs (87% of times)
  src/models/user.rs (62% of times)
  tests/auth_integration.rs (94% of times)
```

### `homer graph`

Explore graph analysis results.

```
homer graph [OPTIONS]

Options:
  --type <TYPE>         Graph type: call, import, combined [default: call]
  --metric <METRIC>     Metric to display: pagerank, betweenness, hits, salience [default: salience]
  --top <N>             Show top N entities [default: 20]
  --community <ID>      Show members of a specific community
  --list-communities    List all detected communities
  --format <FORMAT>     Output format: text, json, dot, mermaid [default: text]

Examples:
  homer graph --metric pagerank --top 10
  homer graph --list-communities
  homer graph --community 3 --format mermaid
  homer graph --type import --format dot > import-graph.dot
```

### `homer diff`

Compare architectural state between two points.

```
homer diff <REF1> <REF2> [OPTIONS]

Arguments:
  <REF1>  Start reference (tag, branch, SHA, snapshot label)
  <REF2>  End reference

Options:
  --format <FORMAT>     Output: text, json, markdown [default: text]
  --include <SECTIONS>  What to include: topology, centrality, communities, coupling

Examples:
  homer diff v1.0 v2.0
  homer diff v1.0 HEAD
  homer diff main~100 main
```

**Output**:
```
Architectural Diff: v1.0 → v2.0

Topology:
  +47 new call edges (12 cross-module)
  -13 removed call edges
  +5 new files, -2 removed files

Centrality Shifts:
  ↑ src/payment/processor.rs: PageRank 0.34 → 0.67 (rising importance)
  ↓ src/legacy/handler.rs: PageRank 0.56 → 0.21 (declining)
  
Community Changes:
  src/auth/oauth.rs moved from community "Core Auth" to "Payment Integration"
  New community detected: "Notification System" (5 files)

Coupling:
  Cross-community edges: 23 → 31 (+35% — coupling increasing)
  New cross-boundary: payment → auth (3 new edges)
```

### `homer serve`

Start MCP server for agent integration.

```
homer serve [OPTIONS]

Options:
  --transport <TYPE>    Transport: stdio, sse [default: stdio]
  --port <PORT>         Port for SSE transport [default: 3000]
  --host <HOST>         Host for SSE transport [default: 127.0.0.1]
```

### `homer snapshot`

Manage graph snapshots.

```
homer snapshot <SUBCOMMAND>

Subcommands:
  create <LABEL>   Create a named snapshot of current graph state
  list             List all snapshots
  delete <LABEL>   Delete a snapshot
```

### `homer status`

Show current state of Homer's knowledge base.

```
homer status

Output:
  Database: .homer/homer.db (4.2 MB)
  Last updated: 2026-02-07T18:30:00Z
  Git checkpoint: abc123f (HEAD~3)
  GitHub checkpoint: PR #142, Issue #87
  
  Nodes: 12,456 (1,247 functions, 342 types, 89 modules, ...)
  Hyperedges: 34,567
  Analysis results: 8,234
  LLM cache entries: 127 (estimated cost: $2.34)
  
  Pending work:
    3 new commits since last update
    2 new PRs since last update
```

---

## MCP Server Tools

The MCP server exposes Homer's query capabilities as tools for AI agents. Built using `rmcp` (Anthropic's Rust MCP SDK), same as [Topos](https://github.com/rand/topos).

### Tool: `homer_query`

```json
{
  "name": "homer_query",
  "description": "Query Homer's repository knowledge base for information about a file, function, or module",
  "inputSchema": {
    "type": "object",
    "properties": {
      "entity": { "type": "string", "description": "File path, function name, or qualified name" },
      "include": { 
        "type": "array", 
        "items": { "type": "string", "enum": ["summary", "metrics", "callers", "callees", "history", "co_changes"] }
      }
    },
    "required": ["entity"]
  }
}
```

### Tool: `homer_graph`

```json
{
  "name": "homer_graph",
  "description": "Query graph metrics. Use to understand code importance and dependencies before making changes.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "metric": { "type": "string", "enum": ["pagerank", "betweenness", "hits", "salience"] },
      "scope": { "type": "string", "description": "File path prefix to scope results" },
      "top_n": { "type": "integer", "default": 10 }
    }
  }
}
```

### Tool: `homer_risk`

```json
{
  "name": "homer_risk",
  "description": "Check risk level for files you're about to modify",
  "inputSchema": {
    "type": "object",
    "properties": {
      "paths": { "type": "array", "items": { "type": "string" }, "description": "File paths to check" }
    },
    "required": ["paths"]
  }
}
```

### Tool: `homer_co_changes`

```json
{
  "name": "homer_co_changes",
  "description": "Find files that typically change together with a given file. Use this to ensure you haven't missed related files.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "min_confidence": { "type": "number", "default": 0.3 }
    },
    "required": ["path"]
  }
}
```

### Tool: `homer_conventions`

```json
{
  "name": "homer_conventions",
  "description": "Get coding conventions for a module or the whole project",
  "inputSchema": {
    "type": "object",
    "properties": {
      "scope": { "type": "string", "description": "Module path or empty for project-wide" }
    }
  }
}
```

---

## Configuration File

Full specification of `.homer/config.toml`:

```toml
[homer]
# Schema version (for migration)
version = "0.1.0"

[analysis]
# Analysis depth: shallow, standard, deep, full
depth = "standard"
# Salience threshold for LLM summarization (0.0 - 1.0)
llm_salience_threshold = 0.7
# Max entities to summarize per LLM run
max_llm_batch_size = 50

[llm]
# Provider: anthropic, openai, custom
provider = "anthropic"
# Model identifier
model = "claude-sonnet-4-20250514"
# API key environment variable
api_key_env = "ANTHROPIC_API_KEY"
# Base URL (for custom providers)
# base_url = "https://api.example.com/v1"
# Max concurrent requests
max_concurrent = 5
# Cost budget per run (USD, 0 = unlimited)
cost_budget = 0.0

[extraction]
# Git history
max_commits = 2000           # 0 = unlimited
# GitHub
github_token_env = "GITHUB_TOKEN"
max_pr_history = 200
max_issue_history = 500
include_comments = true
include_reviews = true

[extraction.structure]
include_patterns = ["**/*.rs", "**/*.py", "**/*.ts", "**/*.tsx", "**/*.js", "**/*.jsx", "**/*.go", "**/*.java"]
exclude_patterns = ["**/node_modules/**", "**/vendor/**", "**/target/**", "**/.git/**", "**/dist/**"]

[extraction.documents]
enabled = true
include_doc_comments = true        # Extract doc comments during graph extraction
include_patterns = [
    "README*", "CONTRIBUTING*", "ARCHITECTURE*", "CHANGELOG*",
    "docs/**/*.md", "doc/**/*.md", "adr/**/*.md", "wiki/**/*.md",
    "*.tps",
]
exclude_patterns = ["**/node_modules/**", "**/vendor/**"]

[extraction.prompts]
enabled = false                     # Opt-in, not opt-out (privacy-sensitive)
sources = ["claude-code", "agent-rules"]
redact_sensitive = true             # Strip API keys, passwords, personal info
store_full_text = false             # Only store extracted metadata, not raw prompt text
hash_session_ids = true             # Don't store raw session identifiers
exclude_contributors = []           # Skip automated agent interactions

[graph]
# Languages to analyze (auto = detect from file tree)
languages = "auto"
# Override tier for specific languages
# [graph.overrides]
# rust = "heuristic"    # Downgrade to heuristic if precise rules are buggy

[graph.snapshots]
# Create snapshots at tagged releases
at_releases = true
# Create snapshots every N commits
every_n_commits = 100

[renderers]
# Which renderers to enable
enabled = ["agents-md", "module-ctx", "risk-map"]

[renderers.agents-md]
output_path = "AGENTS.md"
max_load_bearing = 20
max_change_patterns = 10
max_design_decisions = 10
circularity_mode = "auto"    # auto | diff | merge | overwrite
# auto: write to AGENTS.homer.md if human-curated file exists, else AGENTS.md

[renderers.module-ctx]
filename = ".context.md"
per_directory = true

[renderers.skills]
output_dir = ".claude/skills/"

[renderers.spec]
output_dir = "spec/"
format = "topos"

[renderers.report]
output_path = "homer-report.html"
format = "html"            # html or markdown

[renderers.risk-map]
output_path = "homer-risk.json"

[mcp]
transport = "stdio"
# host = "127.0.0.1"
# port = 3000
```

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Configuration error |
| 3 | Repository not found or not a git repo |
| 4 | Database error (corrupted, incompatible version) |
| 5 | GitHub API error (auth, rate limit) |
| 6 | LLM API error |
| 10 | Partial success (some files failed, results are incomplete) |
