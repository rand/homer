# Troubleshooting

Common issues and solutions when using Homer.

## Installation Issues

### Rust version too old

```
error[E0658]: edition 2024 is unstable and only available on the nightly compiler
```

Homer requires Rust 1.85+. Update your toolchain:

```bash
rustup update stable
rustc --version  # Should show 1.85.0 or later
```

### Build fails with tree-sitter errors

```
error: failed to compile `homer-cli v0.1.0`
```

Tree-sitter grammars require a C compiler. Ensure you have one installed:

- **macOS**: `xcode-select --install`
- **Ubuntu/Debian**: `sudo apt install build-essential`
- **Fedora**: `sudo dnf install gcc`

## Initialization Issues

### "Homer is already initialized"

```
Error: Homer is already initialized in /path/to/repo. Use `homer update` to refresh.
```

Homer found an existing `.homer/config.toml`. To re-initialize from scratch:

```bash
rm -rf .homer
homer init
```

Or use `homer update --force` to re-extract everything while keeping the existing database.

### "Cannot resolve path"

```
Error: Cannot resolve path: ./my-project
```

The path doesn't exist or isn't accessible. Use an absolute path or verify the directory exists:

```bash
ls -la ./my-project
homer init /absolute/path/to/project
```

### "Not a git repository"

Homer requires a git repository. Initialize one if needed:

```bash
cd your-project
git init
git add .
git commit -m "Initial commit"
homer init
```

## Runtime Issues

### "Homer is not initialized"

```
Error: Homer is not initialized in /path/to/repo. Run `homer init` first.
```

Run `homer init` before using other commands. This error appears for `update`, `status`, `query`, `graph`, `diff`, `render`, `snapshot`, `risk-check`, and `serve`.

### Slow initialization on large repos

For repositories with many thousands of commits or files, initialization may take a while. Options:

1. **Reduce depth**: `homer init --depth shallow` processes only the last 500 commits
2. **Limit commits**: Edit `.homer/config.toml` after init:
   ```toml
   [extraction]
   max_commits = 500
   ```
3. **Restrict languages**: `homer init --languages rust,python` to skip languages you don't care about

### "Pipeline execution failed"

The pipeline collects individual failures as warnings without aborting. If you see this error, the entire pipeline failed (not just individual files). Common causes:

- Database file is locked by another process
- Disk is full
- Git repository is corrupted

Check verbose output: `homer init -vv`

### Zero betweenness scores

If `homer graph --metric betweenness` shows all zeros, the import graph may be too sparse for meaningful betweenness computation. This happens when:

- The repository has few files
- Import resolution couldn't resolve most imports (e.g., non-Rust languages with complex module systems)
- Files form a star topology (all importing from one central file) with no bridging

This is expected behavior for small or structurally simple codebases. Betweenness becomes meaningful when files form chains and bridges in the import graph.

### Communities are mostly singletons

If `homer graph --list-communities` shows many single-file communities, the import graph is sparse. Louvain community detection needs a denser graph to find meaningful clusters. This is normal for:

- Small codebases (< 50 files)
- Codebases where most files import from a few central modules (star topology)
- Languages where Homer's import resolution has limited coverage

### Query returns "No entity found"

```
No entity found matching: my_function
```

Homer searches by exact match first, then partial match on the node name. Try:

```bash
# Use the full path for files
homer query src/auth/validate.rs

# Use a more specific name for functions
homer query "AuthService::validate_token"

# Check what nodes Homer has
homer status
```

### Database is large

The `.homer/homer.db` file grows with repository size. For very large repos:

- A 1,000-file repo produces ~5-10 MB
- A 10,000-file repo may produce 50-100 MB

The database uses SQLite WAL mode. Temporary WAL files (`homer.db-wal`, `homer.db-shm`) are created during writes and cleaned up on close.

To compact the database:

```bash
sqlite3 .homer/homer.db "VACUUM;"
```

## MCP Server Issues

### "Unsupported transport"

```
error: invalid value for '--transport <TRANSPORT>': ...
```

Homer supports `stdio` transport only. The MCP server communicates over
stdin/stdout using JSON-RPC.

### MCP server not responding

If the MCP server starts but doesn't respond to queries:

1. Ensure Homer is initialized: `homer status`
2. Check that the database path is correct
3. Try verbose mode: `homer serve --path /your/project -v`

### Configuring MCP in Claude Code

Add to your Claude Code MCP settings:

```json
{
  "mcpServers": {
    "homer": {
      "command": "homer",
      "args": ["serve", "--path", "/absolute/path/to/your/project"]
    }
  }
}
```

Use an absolute path — relative paths may resolve differently depending on the working directory.

## Update Issues

### Update is slower than expected

`homer update` re-runs the full pipeline, but extraction is checkpoint-driven:
- Git extractor processes only commits after `git_last_sha`.
- Structure/document/prompt extractors skip entirely when their checkpoints
  match git HEAD.
- Graph extractor processes only files changed since `graph_last_sha`.

Analysis still recomputes configured metrics each run.

For faster updates:
- Use `--force-analysis` to skip re-extraction and just recompute metrics
- Check if GitHub API calls are timing out (if you have `--no-github` available)

### "Failed to clear checkpoints"

Database may be locked. Ensure no other Homer process is running:

```bash
# Check for running Homer processes
ps aux | grep homer

# If stuck, the WAL file may need cleanup
ls -la .homer/homer.db*
```

## Render Issues

### AGENTS.md is overwritten

Homer regenerates `AGENTS.md` on each run. To preserve human-curated sections, wrap them in preserve markers:

```markdown
<!-- homer:preserve -->
## My Custom Section
This content will be preserved across Homer updates.
<!-- /homer:preserve -->
```

Note the closing tag is `<!-- /homer:preserve -->` (with a forward slash).

### .context.md files cluttering diffs

Add a pattern to your `.gitignore` if you don't want to track them:

```
**/.context.md
```

Or disable the renderer:

```toml
[renderers]
enabled = ["agents-md", "risk-map"]  # Removed "module-ctx"
```

### Render produces empty or minimal output

If `homer render` produces near-empty files:

1. Check that the database has analysis results: `homer status`
2. If only nodes/edges exist but no analyses, run: `homer update --force-analysis`
3. For specific renderers, ensure they're enabled in config or use `--format <name>`

### Unknown renderer name

```
Error: Unknown renderer: my-renderer
```

Valid renderer names: `agents-md`, `module-ctx`, `risk-map`, `skills`, `topos-spec`, `report`.

## Snapshot Issues

### "Snapshot already exists"

Snapshot labels must be unique. Use `homer snapshot list` to see existing labels, then choose a different name.

### Snapshot diff shows no changes

If two snapshots were created close together without new commits between them, the diff will be empty. Snapshots capture the graph state at a point in time — if the graph hasn't changed, neither has the snapshot.

## Risk Check Issues

### Risk check fails in CI

`homer risk-check` exits with a non-zero code when files exceed the threshold. This is by design — it's a CI gate. To fix:

1. Lower the threshold: `homer risk-check --threshold 0.9`
2. Address the risk: improve bus factor (get more reviewers) or reduce change frequency
3. Filter to specific paths: `homer risk-check --filter src/critical/`

### All files show zero risk

If no analysis data exists, risk scores default to zero. Ensure you've run the full pipeline:

```bash
homer update --force-analysis
homer risk-check
```

## GitHub/GitLab API Issues

### "GITHUB_TOKEN not set" or extraction skipped

GitHub extraction requires a personal access token:

```bash
export GITHUB_TOKEN=ghp_your_token_here
homer update --force
```

The token needs `repo` scope for private repositories, or just `public_repo` for public ones.

### Rate limiting

```
Error: GitHub API rate limit exceeded
```

GitHub's API rate limit is 5,000 requests/hour with authentication. For large repos with many PRs:

1. Reduce PR history: set `extraction.github.max_pr_history` in config
2. Use `--depth shallow` to skip GitHub extraction entirely
3. Wait for the rate limit to reset (check `X-RateLimit-Reset` header)

### GitLab token permissions

GitLab tokens need `read_api` scope. For private projects, you may also need `read_repository`.

```bash
export GITLAB_TOKEN=glpat-your_token_here
homer update --force
```

## LLM Integration Issues

### "Environment variable not set"

```
Skipping semantic analysis (LLM provider unavailable)
```

This is informational, not an error. Set the API key to enable LLM features:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

### LLM costs higher than expected

Check your `cost_budget` setting:

```toml
[llm]
cost_budget = 2.0  # $2 max per run
```

Also check `analysis.llm_salience_threshold` — lowering it sends more entities to the LLM:

```toml
[analysis]
llm_salience_threshold = 0.8  # Only the most important entities
```

### Refreshing stale LLM summaries

After upgrading the LLM model or changing prompts:

```bash
homer update --force-semantic
```

This clears only LLM-derived results and regenerates them.

## Getting Help

For bugs and feature requests, open an issue at: https://github.com/rand/homer/issues

For verbose diagnostic output, run any command with `-vvv`:

```bash
homer init -vvv 2> homer-debug.log
```

This captures trace-level logging to a file for debugging.

## Next Steps

- [CLI Reference](cli-reference.md) — Full command reference
- [Configuration](configuration.md) — All config options
- [MCP Integration](mcp-integration.md) — AI tool integration
- [Getting Started](getting-started.md) — First run walkthrough
