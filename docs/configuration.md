# Configuration Reference

Homer stores its configuration in `.homer/config.toml`, created during `homer init`. This document covers every configuration option.

## Sections

- [analysis](#analysis) — Depth and LLM gating
- [extraction](#extraction) — Git, structure, document, prompt extraction
- [graph](#graph) — Language selection
- [renderers](#renderers) — Output artifact control
- [llm](#llm) — LLM provider settings

## Full Default Configuration

```toml
[homer]
version = "0.1.0"

[analysis]
depth = "standard"
llm_salience_threshold = 0.7
max_llm_batch_size = 50

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

[extraction.gitlab]
token_env = "GITLAB_TOKEN"
max_mr_history = 500
max_issue_history = 1000
include_comments = true
include_reviews = true

[graph]
languages = "auto"

[renderers]
enabled = ["agents-md", "module-ctx", "risk-map"]

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
max_concurrent = 5
cost_budget = 0.0
enabled = false
```

---

## `[analysis]`

Controls analysis behavior and depth.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `depth` | string | `"standard"` | Analysis depth level (see below) |
| `llm_salience_threshold` | float | `0.7` | Minimum salience score for LLM enrichment |
| `max_llm_batch_size` | integer | `50` | Max entities to send to LLM per run |

### Depth Levels

| Level | Git History | Description |
|-------|-----------|-------------|
| `shallow` | Last 500 commits | Fast analysis for large repos |
| `standard` | Last 2000 commits | Good balance of speed and coverage |
| `deep` | All commits | Full history analysis |
| `full` | All commits | Everything including LLM enrichment |

Set via config or CLI flag: `homer init --depth deep`.

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

### `[extraction.gitlab]`

Controls GitLab API extraction (for GitLab-hosted repositories).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `token_env` | string | `"GITLAB_TOKEN"` | Environment variable holding the GitLab PAT |
| `max_mr_history` | integer | `500` | Max merge requests to fetch |
| `max_issue_history` | integer | `1000` | Max issues to fetch |
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

Disable specific renderers:

```toml
[renderers]
enabled = ["agents-md"]  # Only generate AGENTS.md
```

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

---

## CLI Overrides

Some configuration options can be overridden via CLI flags:

| Config | CLI Flag | Command |
|--------|----------|---------|
| `analysis.depth` | `--depth <level>` | `homer init` |
| `graph.languages` | `--languages <list>` | `homer init` |
| n/a | `--no-github` | `homer init` |
| n/a | `--no-llm` | `homer init` |
| n/a | `--force` | `homer update` |
| n/a | `--force-analysis` | `homer update` |
| n/a | `--db-path <path>` | `homer init` |

---

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `ANTHROPIC_API_KEY` | API key for Anthropic LLM provider |
| `OPENAI_API_KEY` | API key for OpenAI LLM provider |
| `GITHUB_TOKEN` | GitHub API token for PR/issue extraction |
| `GITLAB_TOKEN` | GitLab API token for MR/issue extraction |
| `RUST_LOG` | Fine-grained logging control (e.g., `homer_core=debug`) |

---

## Next Steps

- [Getting Started](getting-started.md) — First run walkthrough
- [Concepts](concepts.md) — How the pipeline works
- [Troubleshooting](troubleshooting.md) — Common issues
