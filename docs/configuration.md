# Configuration Reference

Homer stores its configuration in `.homer/config.toml`, created during `homer init`. This document covers every configuration option.

## Sections

- [homer](#homer) — Version and database path
- [analysis](#analysis) — Depth, LLM gating, invalidation policy
- [extraction](#extraction) — Git, structure, document, prompt, GitHub, GitLab extraction
- [graph](#graph) — Language selection and snapshot policy
- [renderers](#renderers) — Output artifact control and per-renderer configuration
- [llm](#llm) — LLM provider settings
- [mcp](#mcp) — MCP server transport

## Full Default Configuration

```toml
[homer]
version = "0.1.0"

[analysis]
depth = "standard"
llm_salience_threshold = 0.7
max_llm_batch_size = 50

[analysis.invalidation]
global_centrality_on_topology_change = true
conservative_semantic_invalidation = true

[extraction]
max_commits = 2000

[extraction.structure]
include_patterns = [
    "**/*.rs", "**/*.py", "**/*.ts", "**/*.tsx",
    "**/*.js", "**/*.jsx", "**/*.go", "**/*.java",
]
exclude_patterns = [
    "**/node_modules/**", "**/vendor/**", "**/target/**",
    "**/.git/**", "**/dist/**",
]

[extraction.documents]
enabled = true
include_doc_comments = true
include_patterns = [
    "README*", "CONTRIBUTING*", "ARCHITECTURE*", "CHANGELOG*",
    "docs/**/*.md", "doc/**/*.md", "adr/**/*.md",
]
exclude_patterns = ["**/node_modules/**", "**/vendor/**"]

[extraction.prompts]
enabled = false
sources = ["claude-code", "agent-rules"]
redact_sensitive = true
store_full_text = false
hash_session_ids = true

[extraction.github]
token_env = "GITHUB_TOKEN"
max_pr_history = 500
max_issue_history = 1000
include_comments = true
include_reviews = true

[extraction.gitlab]
token_env = "GITLAB_TOKEN"
max_mr_history = 500
max_issue_history = 1000
include_comments = true
include_reviews = true

[graph]
languages = "auto"

[graph.snapshots]
at_releases = true
every_n_commits = 100

[renderers]
enabled = ["agents-md", "module-ctx", "risk-map"]

[renderers.agents-md]
output_path = "AGENTS.md"
max_load_bearing = 20
max_change_patterns = 10
max_design_decisions = 10
circularity_mode = "auto"

[renderers.module-ctx]
filename = ".context.md"
per_directory = true

[renderers.skills]
output_dir = ".claude/skills/"

[renderers.topos-spec]
output_dir = "spec/"
format = "topos"

[renderers.report]
output_path = "homer-report.html"
format = "html"

[renderers.risk-map]
output_path = "homer-risk.json"

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
max_concurrent = 5
cost_budget = 0.0
enabled = false

[mcp]
transport = "stdio"
```

---

## `[homer]`

Top-level metadata.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `version` | string | `"0.1.0"` | Config schema version |
| `db_path` | string | none | Custom database location (overrides default `.homer/homer.db`) |

---

## `[analysis]`

Controls analysis behavior and depth.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `depth` | string | `"standard"` | Analysis depth level (see below) |
| `llm_salience_threshold` | float | `0.7` | Minimum salience score for LLM enrichment |
| `max_llm_batch_size` | integer | `50` | Max entities to send to LLM per run |

### Depth Levels

Each depth level overrides extraction and analysis limits:

| Level | Git History | GitHub PRs | GitHub Issues | LLM Batch |
|-------|-----------|------------|---------------|-----------|
| `shallow` | Last 500 commits | 0 (skip) | 0 (skip) | 0 |
| `standard` | Last 2000 commits | 200 | 500 | 50 |
| `deep` | All commits | 500 | 1000 | 200 |
| `full` | All commits | Unlimited | Unlimited | Config value |

Set via config or CLI flag: `homer init --depth deep`.

### `[analysis.invalidation]`

Controls how analysis results are invalidated when the graph changes.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `global_centrality_on_topology_change` | bool | `true` | Any graph topology change invalidates all centrality scores (PageRank, betweenness, HITS, composite salience) |
| `conservative_semantic_invalidation` | bool | `true` | Only invalidate semantic summaries when a node's own content hash changes, not when neighbors change |

The defaults are conservative: centrality is globally recomputed on any topology change (correct, since PageRank is a global property), while LLM-derived summaries are only refreshed when the summarized code itself changes (saving API costs).

---

## `[extraction]`

Controls what data Homer extracts from the repository.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max_commits` | integer | `2000` | Maximum commits to process (0 = unlimited) |

### `[extraction.structure]`

Controls file tree scanning.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `include_patterns` | array of strings | See above | Glob patterns for source files to analyze |
| `exclude_patterns` | array of strings | See above | Glob patterns for files/directories to skip |

Add patterns for additional languages or remove languages you don't need:

```toml
[extraction.structure]
include_patterns = ["**/*.rs", "**/*.py"]  # Only Rust and Python
exclude_patterns = ["**/target/**", "**/tests/**"]
```

### `[extraction.documents]`

Controls documentation extraction.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Whether to extract documentation files |
| `include_doc_comments` | bool | `true` | Extract doc comments from source files during graph extraction |
| `include_patterns` | array of strings | See above | Glob patterns for documentation files |
| `exclude_patterns` | array of strings | See above | Glob patterns for docs to skip |

### `[extraction.prompts]`

Controls AI agent interaction mining. **Disabled by default** for privacy.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Enable prompt extraction (opt-in) |
| `sources` | array of strings | `["claude-code", "agent-rules"]` | What to extract |
| `redact_sensitive` | bool | `true` | Strip API keys, passwords, personal info |
| `store_full_text` | bool | `false` | Store raw prompt text (default: metadata only) |
| `hash_session_ids` | bool | `true` | Hash session identifiers for privacy |

### `[extraction.github]`

Controls GitHub API extraction (for GitHub-hosted repositories). Requires `GITHUB_TOKEN`.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `token_env` | string | `"GITHUB_TOKEN"` | Environment variable holding the GitHub PAT |
| `max_pr_history` | integer | `500` | Max pull requests to fetch (0 = unlimited) |
| `max_issue_history` | integer | `1000` | Max issues to fetch (0 = unlimited) |
| `include_comments` | bool | `true` | Include PR/issue comments as metadata |
| `include_reviews` | bool | `true` | Include PR reviews and create Reviewed edges |

GitHub extraction creates PullRequest and Issue nodes, along with Resolves edges (PR → issue) and Reviewed edges (contributor → PR).

### `[extraction.gitlab]`

Controls GitLab API extraction (for GitLab-hosted repositories). Requires `GITLAB_TOKEN`.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `token_env` | string | `"GITLAB_TOKEN"` | Environment variable holding the GitLab PAT |
| `max_mr_history` | integer | `500` | Max merge requests to fetch (0 = unlimited) |
| `max_issue_history` | integer | `1000` | Max issues to fetch (0 = unlimited) |
| `include_comments` | bool | `true` | Include MR comments |
| `include_reviews` | bool | `true` | Include approvals/reviews |

---

## `[graph]`

Controls graph extraction (call graphs, import graphs).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `languages` | string or array | `"auto"` | Languages to analyze |

Set to `"auto"` to detect languages from file extensions, or provide an explicit list:

```toml
[graph]
languages = ["rust", "python", "typescript"]
```

Supported language identifiers: `rust`, `python`, `typescript`, `javascript`, `go`, `java`.

### `[graph.snapshots]`

Controls automatic graph snapshot creation. Snapshots capture the graph state at a point in time, enabling `homer snapshot diff` comparisons.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `at_releases` | bool | `true` | Create a snapshot at each tagged release |
| `every_n_commits` | integer | `100` | Create a snapshot every N commits (0 = disabled) |

Release snapshots use the release tag as their label (e.g., `v1.0.0`). Commit-count snapshots use `auto-N` labels (e.g., `auto-100`, `auto-200`).

```toml
[graph.snapshots]
at_releases = true
every_n_commits = 50  # Snapshot every 50 commits
```

---

## `[renderers]`

Controls which output artifacts Homer generates.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | array of strings | `["agents-md", "module-ctx", "risk-map"]` | Which renderers to run |

Available renderers:

| Renderer ID | Output | Description |
|------------|--------|-------------|
| `agents-md` | `AGENTS.md` | Context file for AI coding agents |
| `module-ctx` | `*/.context.md` | Per-directory context files |
| `risk-map` | `homer-risk.json` | Machine-readable risk annotations |
| `skills` | `.claude/skills/*.md` | Claude Code skill files |
| `topos-spec` | `spec/*.toml` | Topological specification files |
| `report` | `homer-report.html` | Human-readable analysis report |

Disable specific renderers or enable all 6:

```toml
[renderers]
enabled = ["agents-md"]  # Only generate AGENTS.md

# Or enable everything:
enabled = ["agents-md", "module-ctx", "risk-map", "skills", "topos-spec", "report"]
```

### `[renderers.agents-md]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_path` | string | `"AGENTS.md"` | Output file path relative to repo root |
| `max_load_bearing` | integer | `20` | Max entries in the Load-Bearing Code table |
| `max_change_patterns` | integer | `10` | Max entries in the Change Patterns tables |
| `max_design_decisions` | integer | `10` | Max entries in the Key Design Decisions list |
| `circularity_mode` | string | `"auto"` | How to handle existing AGENTS.md: `auto`, `diff`, `merge`, `overwrite` |

### `[renderers.module-ctx]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `filename` | string | `".context.md"` | Filename for per-directory context files |
| `per_directory` | bool | `true` | Whether to generate one file per directory |

### `[renderers.skills]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_dir` | string | `".claude/skills/"` | Output directory for skill files |

### `[renderers.topos-spec]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_dir` | string | `"spec/"` | Output directory for spec files |
| `format` | string | `"topos"` | Spec format |

### `[renderers.report]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_path` | string | `"homer-report.html"` | Output file path |
| `format` | string | `"html"` | Report format: `html` or `markdown` |

### `[renderers.risk-map]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_path` | string | `"homer-risk.json"` | Output file path |

---

## `[llm]`

Controls LLM integration for semantic enrichment. **Disabled by default.**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `provider` | string | `"anthropic"` | LLM provider: `anthropic`, `openai`, `custom` |
| `model` | string | `"claude-sonnet-4-20250514"` | Model identifier |
| `api_key_env` | string | `"ANTHROPIC_API_KEY"` | Environment variable for API key |
| `base_url` | string | none | Base URL override (for custom providers) |
| `max_concurrent` | integer | `5` | Max concurrent LLM requests |
| `cost_budget` | float | `0.0` | USD budget per run (0 = unlimited) |
| `enabled` | bool | `false` | Whether LLM features are active |

To enable LLM enrichment:

```toml
[llm]
enabled = true
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
cost_budget = 5.0  # $5 max per run
```

LLM enrichment is gated by salience — only entities above `analysis.llm_salience_threshold` are sent to the LLM. Entities with quality doc comments may skip LLM summarization entirely.

The semantic analyzer produces three analysis kinds: `SemanticSummary`, `DesignRationale`, and `InvariantDescription`. It is automatically skipped at `shallow` depth or when `llm.enabled = false`.

---

## `[mcp]`

Controls the MCP (Model Context Protocol) server, started via `homer serve`.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `transport` | string | `"stdio"` | Transport type (`stdio` only) |

Only `stdio` transport is supported.
For backward compatibility, legacy `transport = "sse"` is accepted and mapped to
`stdio`.

```toml
[mcp]
transport = "stdio"
```

---

## CLI Overrides

Some configuration options can be overridden via CLI flags:

| Config | CLI Flag | Command |
|--------|----------|---------|
| `analysis.depth` | `--depth <level>` | `homer init` |
| `graph.languages` | `--languages <list>` | `homer init` |
| `homer.db_path` | `--db-path <path>` | `homer init` |
| n/a | `--no-github` | `homer init` |
| n/a | `--no-llm` | `homer init` |
| n/a | `--force` | `homer update` |
| n/a | `--force-analysis` | `homer update` |
| n/a | `--force-semantic` | `homer update` |
| `mcp.transport` | `--transport <type>` | `homer serve` |

---

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `ANTHROPIC_API_KEY` | API key for Anthropic LLM provider |
| `OPENAI_API_KEY` | API key for OpenAI LLM provider |
| `GITHUB_TOKEN` | GitHub API token for PR/issue extraction |
| `GITLAB_TOKEN` | GitLab API token for MR/issue extraction |
| `HOMER_DB_PATH` | Override database location (lower priority than `--db-path`) |
| `RUST_LOG` | Fine-grained logging control (e.g., `homer_core=debug`) |

---

## Next Steps

- [CLI Reference](cli-reference.md) — Full command reference
- [Getting Started](getting-started.md) — First run walkthrough
- [Concepts](concepts.md) — How the pipeline works
- [Troubleshooting](troubleshooting.md) — Common issues
