# homer-mcp

MCP (Model Context Protocol) server for Homer. Exposes codebase analysis as tools that AI agents can call during their work.

## Tools

| Tool | Description |
|------|-------------|
| `homer_query` | Look up entities by name — returns metadata and salience |
| `homer_graph` | Centrality metrics for top entities |
| `homer_risk` | Per-file risk assessment (salience, bus factor, change frequency) |
| `homer_co_changes` | Files that frequently change together |
| `homer_conventions` | Project conventions (naming, testing, error handling, docs) |

## Architecture

Built on [rmcp](https://docs.rs/rmcp) using the `#[tool_router]` and `#[tool]` macros. The server wraps an `Arc<SqliteStore>` for thread-safe access.

Each tool method is separated into a `do_*` method for testability — unit tests call these directly without MCP transport.

```
lib.rs
  HomerMcpServer           — Server struct with tool router
  QueryParams/GraphParams/… — schemars-annotated parameter types
  do_query/do_graph/…       — Tool logic (separated from dispatch)
  serve_stdio()             — Entry point for stdio transport
  resolve_db_path()         — DB path resolution
```

## Transport

Currently supports `stdio` transport only. The server communicates over stdin/stdout using JSON-RPC.

## Tests

7 unit tests covering tool logic with in-memory store.

## Documentation

See [docs/mcp-integration.md](../docs/mcp-integration.md) for setup guides and usage examples.
