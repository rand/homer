# Cookbook

Practical recipes for common Homer workflows. This assumes you've read [Getting Started](getting-started.md) and have a working Homer installation.

## CI Integration

### GitHub Actions: Risk Gate

Add Homer as a quality gate in your CI pipeline. This fails the build if any file exceeds a risk threshold:

```yaml
name: Homer Risk Check
on: [pull_request]

jobs:
  risk-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Homer needs git history

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install Homer
        run: cargo install --path homer-cli

      - name: Initialize Homer
        run: homer init --depth shallow --no-github --no-llm

      - name: Risk Check
        run: homer risk-check --threshold 0.7 --format json
```

For repos that already have a `.homer/homer.db` committed or cached:

```yaml
      - name: Update Homer
        run: homer update

      - name: Risk Check
        run: homer risk-check --threshold 0.7
```

### GitLab CI: Risk Gate

```yaml
homer-risk:
  stage: test
  script:
    - cargo install --path homer-cli
    - homer init --depth shallow --no-github --no-llm
    - homer risk-check --threshold 0.7
  rules:
    - if: '$CI_PIPELINE_SOURCE == "merge_request_event"'
```

### Caching the Database

For faster CI runs, cache Homer's database between builds:

```yaml
      - name: Cache Homer DB
        uses: actions/cache@v4
        with:
          path: .homer/homer.db
          key: homer-${{ runner.os }}-${{ hashFiles('Cargo.lock', 'package-lock.json') }}
          restore-keys: homer-${{ runner.os }}-

      - name: Update or Initialize
        run: |
          if [ -f .homer/homer.db ]; then
            homer update
          else
            homer init --depth shallow --no-github --no-llm
          fi
```

## PR Review Workflow

### Assessing PR Impact

Before reviewing a PR, use `homer diff` to understand the architectural impact:

```bash
# Compare the PR branch against main
homer diff main feature-branch --format markdown
```

This shows:
- Which high-salience files are touched (changes to load-bearing code)
- Whether low bus-factor files are modified (single-contributor risk)
- Which communities are affected (cross-cutting changes)
- What modules are impacted

### Checking for Missing Co-Changes

One of the most valuable Homer insights: if file A changed but its co-change partner B didn't, that's a potential bug:

```bash
# Check what usually changes with the modified files
homer query src/store/sqlite.rs --include co_changes --format json
```

If the PR touches `sqlite.rs` but not `traits.rs` or `schema.rs`, and those files have high co-change confidence, the PR may be missing necessary changes.

## Team Onboarding

### Generating Orientation Materials

When a new developer joins, generate a comprehensive overview:

```bash
# Initialize Homer on the project
homer init

# Show the most important files
homer graph --metric salience --top 30

# Show the project's communities (architectural modules)
homer graph --list-communities

# Generate a full report
homer render --format report
```

The generated `AGENTS.md` is also useful for humans — it's a concise summary of the project's architecture, conventions, and danger zones.

### Understanding a Specific Area

When onboarding someone into a specific module:

```bash
# What's important in this directory?
homer graph --metric salience --top 10 --path /path/to/project

# What does this key file do? Who works on it?
homer query src/auth/middleware.rs

# What changes with it?
homer query src/auth/middleware.rs --include co_changes

# Deep dive with callers and callees
homer query src/auth/middleware.rs --include callers,callees --depth 2
```

## Monitoring Code Health

### Tracking Salience Trends

Create snapshots at regular intervals to track how your codebase evolves:

```bash
# Automatic: configure in .homer/config.toml
[graph.snapshots]
at_releases = true
every_n_commits = 100

# Manual: create named snapshots at milestones
homer snapshot create pre-refactor
# ... do the refactor ...
homer update
homer snapshot create post-refactor
```

### Identifying Architectural Drift

Compare snapshots to see what's changed structurally:

```bash
homer snapshot list
homer diff v1.0 v2.0 --include centrality,communities
```

Look for:
- Files whose salience increased significantly (becoming load-bearing without anyone noticing)
- New cross-community dependencies (architectural coupling creeping in)
- Bus factor decreasing on critical files (knowledge concentration)

### Weekly Health Check

Add a weekly cron job to generate updated analysis:

```bash
#!/bin/bash
cd /path/to/project
homer update
homer risk-check --threshold 0.8 --format json > /tmp/homer-risk.json
homer graph --metric salience --top 20 --format json > /tmp/homer-salience.json
```

## Large Repo Optimization

### Depth Tuning

For monorepos or repositories with 50,000+ commits:

```bash
# Start shallow — see results in seconds
homer init --depth shallow

# If you need more history for co-change detection, go standard
homer update --force
# Edit .homer/config.toml: depth = "standard"
homer update --force-analysis
```

### Language Filtering

Skip languages you don't care about:

```bash
# Only analyze Rust and TypeScript
homer init --languages rust,typescript

# Or configure after init:
# .homer/config.toml
[graph]
languages = ["rust", "typescript"]
```

### Selective Rendering

Don't generate artifacts you won't use:

```toml
# .homer/config.toml
[renderers]
enabled = ["agents-md", "risk-map"]  # Skip module-ctx, skills, etc.
```

Or render on demand:

```bash
# Only render AGENTS.md
homer render --format agents-md

# Render everything once for a report, then go back to selective
homer render --all
```

## Custom Renderer Configuration

### Tuning AGENTS.md

Control how much data goes into AGENTS.md:

```toml
[renderers.agents-md]
max_load_bearing = 30      # Show more important files (default: 20)
max_change_patterns = 15   # Show more co-change patterns (default: 10)
max_design_decisions = 5   # Fewer design decisions (default: 10)
circularity_mode = "merge" # How to handle existing content
```

### Preserving Human Content

Add `<!-- homer:preserve -->` markers in AGENTS.md to protect hand-written sections:

```markdown
## Build & Test
<!-- homer:auto -->
(This section is regenerated by Homer)

<!-- homer:preserve -->
## Team Conventions
These are team-specific conventions that Homer can't detect:
- Always use feature branches
- PR descriptions must reference a ticket
<!-- /homer:preserve -->
```

### Custom Context Filenames

```toml
[renderers.module-ctx]
filename = ".module-context.md"  # Instead of .context.md
per_directory = true
```

## LLM Enrichment

### Setting Up Semantic Analysis

```toml
[llm]
enabled = true
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
cost_budget = 5.0  # $5 max per run

[analysis]
depth = "standard"
llm_salience_threshold = 0.7  # Only summarize high-salience entities
max_llm_batch_size = 50       # Entities per run
```

```bash
export ANTHROPIC_API_KEY=sk-ant-...
homer update --force-analysis
```

### Budget Control

The `cost_budget` setting caps spending per run. Set it low during experimentation:

```toml
[llm]
cost_budget = 1.0  # $1 max — good for trying it out
```

### Refreshing Summaries

After a model upgrade or when summaries feel stale:

```bash
homer update --force-semantic
```

This clears only LLM-derived results (SemanticSummary, DesignRationale, InvariantDescription) and regenerates them, without re-extracting or recomputing other analyses.

### Using a Custom Provider

```toml
[llm]
provider = "custom"
model = "my-local-model"
api_key_env = "MY_API_KEY"
base_url = "http://localhost:8080/v1"
```

## Next Steps

- [CLI Reference](cli-reference.md) — Full command reference
- [Configuration](configuration.md) — All config options
- [MCP Integration](mcp-integration.md) — AI tool integration
- [Extending Homer](extending.md) — Adding languages and analyzers
