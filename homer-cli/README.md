# homer-cli

The `homer` command-line binary. Built with [clap](https://docs.rs/clap) for argument parsing and [indicatif](https://docs.rs/indicatif) for progress display.

## Commands

| Command | File | Description |
|---------|------|-------------|
| `homer init` | `commands/init.rs` | First-time full analysis |
| `homer update` | `commands/update.rs` | Incremental update |
| `homer status` | `commands/status.rs` | Database stats and checkpoints |
| `homer query` | `commands/query.rs` | Entity lookup with metrics |
| `homer graph` | `commands/graph.rs` | Graph rankings and visualization |
| `homer diff` | `commands/diff.rs` | Architectural diff between refs |
| `homer render` | `commands/render.rs` | Selective artifact generation |
| `homer snapshot` | `commands/snapshot.rs` | Snapshot management (create/list/delete) |
| `homer risk-check` | `commands/risk_check.rs` | CI risk gate |
| `homer serve` | `commands/serve.rs` | MCP server startup |

## Shared Utilities

`commands/mod.rs` provides:
- `load_config(repo_path)` — Reads `.homer/config.toml`
- `resolve_db_path(repo_path)` — Priority: `HOMER_DB_PATH` env > `.homer/homer.db`

## Entry Point

`main.rs` sets up logging (via `tracing-subscriber`), parses args, and dispatches to the appropriate command handler.

## Tests

11 unit tests for argument parsing and output formatting.

## Documentation

See [docs/cli-reference.md](../docs/cli-reference.md) for the complete user-facing reference.
