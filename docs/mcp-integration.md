# MCP Integration

Homer exposes its analysis as an [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, allowing AI coding agents to query the knowledge base directly during their work. Instead of reading a static `AGENTS.md`, agents can ask Homer targeted questions: "What are the risk factors for this file?", "What files co-change with this one?", "What conventions does this project follow?"

## Setup

### Claude Code

Add to your Claude Code MCP settings (`.claude/settings.json` or via the Claude Code UI):

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

### Cursor

Add to your Cursor MCP configuration (`.cursor/mcp.json`):

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

### Other MCP-Compatible Tools

Any tool that supports MCP stdio transport can use Homer. The server communicates over stdin/stdout using JSON-RPC. Start it manually for testing:

```bash
homer serve --path /path/to/project
```

## Tools

Homer's MCP server exposes 5 tools. Each returns JSON.

### `homer_query`

Look up entities (functions, types, files, modules) by name. Returns metadata and salience data.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `entity` | string | Yes | Entity name or substring to search for |
| `kind` | string | No | Kind filter: `function`, `type`, `file`, `module` |
| `include` | array of strings | No | Sections: `summary`, `metrics`, `callers`, `callees`, `history`, `co_changes` |

**Example request:**

```json
{
  "entity": "validate_token",
  "kind": "function",
  "include": ["metrics", "callers"]
}
```

**Example response:**

```json
{
  "count": 1,
  "results": [
    {
      "name": "AuthService::validate_token",
      "kind": "Function",
      "salience": {
        "score": 0.82,
        "classification": "ActiveHotspot"
      },
      "callers": ["handle_request", "middleware::auth"]
    }
  ]
}
```

### `homer_graph`

Get centrality metrics for top entities. Identifies load-bearing code, structural bottlenecks, and architectural hubs.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `top` | integer | No | Number of top entities (default: 10) |
| `metric` | string | No | `pagerank`, `betweenness`, `hits`, `salience` (default: `salience`) |
| `scope` | string | No | File path prefix to scope results (e.g., `src/core/`) |

**Example request:**

```json
{
  "metric": "salience",
  "top": 5,
  "scope": "src/store/"
}
```

**Example response:**

```json
{
  "metric": "salience",
  "count": 5,
  "results": [
    {
      "name": "src/store/sqlite.rs",
      "score": 0.91,
      "data": {
        "score": 0.91,
        "classification": "FoundationalStable"
      }
    }
  ]
}
```

### `homer_risk`

Assess risk factors for one or more files. Returns change frequency, bus factor, salience, community, and an overall risk level.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `paths` | array of strings | Yes | File paths relative to repo root |

**Example request:**

```json
{
  "paths": ["src/auth/validate.rs", "src/store/sqlite.rs"]
}
```

**Example response:**

```json
{
  "count": 2,
  "results": [
    {
      "file": "src/auth/validate.rs",
      "salience": { "score": 0.72 },
      "contributor_concentration": { "bus_factor": 1 },
      "change_frequency": { "total": 25 },
      "risk_level": "critical"
    },
    {
      "file": "src/store/sqlite.rs",
      "salience": { "score": 0.91 },
      "contributor_concentration": { "bus_factor": 3 },
      "change_frequency": { "total": 8 },
      "risk_level": "medium"
    }
  ]
}
```

Risk levels: `low` (0–1 points), `medium` (2–3), `high` (4–5), `critical` (6+). Points come from salience (0–3), bus factor (0–2), and change frequency (0–2).

### `homer_co_changes`

Find files that frequently change together. Use when planning modifications to understand ripple effects.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | No | File to find co-change partners for (omit for top global pairs) |
| `top` | integer | No | Maximum pairs to return (default: 10) |
| `min_confidence` | float | No | Minimum confidence threshold (default: 0.3) |

**Example request:**

```json
{
  "path": "src/store/sqlite.rs",
  "top": 5
}
```

**Example response:**

```json
{
  "count": 3,
  "results": [
    {
      "files": ["src/store/sqlite.rs", "src/store/traits.rs"],
      "confidence": 0.78,
      "co_occurrences": 15
    },
    {
      "files": ["src/store/sqlite.rs", "src/store/schema.rs"],
      "confidence": 0.65,
      "co_occurrences": 11
    }
  ]
}
```

### `homer_conventions`

Get project conventions (naming, testing, error handling, documentation). Use to understand and follow established patterns.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `category` | string | No | `naming`, `testing`, `error_handling`, `documentation`, `agent_rules` |
| `scope` | string | No | Module path to scope conventions to |

**Example request:**

```json
{
  "category": "naming"
}
```

**Example response:**

```json
{
  "count": 1,
  "categories": {
    "naming": {
      "count": 42,
      "patterns": [
        {
          "file": "src/store/sqlite.rs",
          "data": { "style": "snake_case", "consistency": 0.95 }
        }
      ]
    }
  }
}
```

## Workflow Examples

### Before Modifying a File

An AI agent about to modify `src/auth/validate.rs` can:

1. **Check risk**: Call `homer_risk` with the file path to understand impact
2. **Find co-changes**: Call `homer_co_changes` to see what other files typically change alongside it
3. **Check conventions**: Call `homer_conventions` to match the project's patterns

### Understanding a Module

When an agent needs to understand `src/store/`:

1. **Graph overview**: Call `homer_graph` with `scope: "src/store/"` to see the most important files
2. **Query specifics**: Call `homer_query` on the top-ranked file to see callers, callees, and metrics

### PR Risk Assessment

Before approving a PR that touches multiple files:

1. **Risk check**: Call `homer_risk` with all modified file paths
2. **Co-change analysis**: Call `homer_co_changes` for each modified file to see if expected co-changes are missing (a forgotten file is often a source of bugs)
3. **Diff context**: The agent can combine Homer's risk data with the actual diff for informed review

## Troubleshooting

### Server Not Responding

1. Ensure Homer is initialized: `homer status`
2. Verify the database exists: `ls .homer/homer.db`
3. Test the server directly: `echo '{}' | homer serve --path /your/project`
4. Try verbose mode: `homer serve --path /your/project -v`

### "Unsupported Transport"

Currently only `stdio` transport is supported. If your MCP client requires SSE, this is not yet implemented.

### No Data Returned

If tools return empty results:
- Run `homer update` to ensure the database is populated
- Check `homer status` to verify nodes and analyses exist
- For convention data, make sure the pipeline has run analysis (not just extraction)

### Connection Issues in Claude Code

- Use absolute paths in the MCP configuration
- Ensure `homer` is in your `PATH` (try `which homer`)
- Check Claude Code's MCP logs for connection errors

## Next Steps

- [CLI Reference](cli-reference.md) — Full command reference
- [Cookbook](cookbook.md) — CI integration and workflow recipes
- [Configuration](configuration.md) — MCP section configuration
- [Troubleshooting](troubleshooting.md) — General troubleshooting
