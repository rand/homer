# ADR 0003: MCP Transport Strategy — Stdio Only

## Status
Accepted

## Date
2026-02-20

## Context
CLI/config/docs exposed `sse` as a selectable MCP transport, but runtime
implementation only supported `stdio`. This produced avoidable user-visible
mismatch and configuration ambiguity.

## Decision
De-surface SSE and standardize Homer MCP transport on `stdio` only.

Implementation scope:
- Config transport type narrowed to stdio-only (`McpTransport::Stdio`)
- Legacy `transport = "sse"` is accepted as a compatibility alias and mapped to
  stdio
- CLI `homer serve --transport` accepts only `stdio`
- Docs/specs updated to remove SSE as an active option

## Consequences
- No runtime/config contradiction remains for transport support.
- Existing configs using `sse` do not break hard; they normalize to stdio.
- Future SSE support requires a new ADR and explicit implementation work.
