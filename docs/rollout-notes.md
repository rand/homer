# Rollout and Compatibility Notes

## Scope
This release aligns Homer on canonical cross-component contracts and idempotent
incremental extraction behavior.

## What Changed

1. Canonical contract enforcement:
- Canonical roles/keys are centralized in `homer-core/src/contracts.rs`.
- Legacy read aliases remain supported during the current compatibility window.

2. Hyperedge idempotency:
- Hyperedges now use deterministic semantic identity keys.
- Legacy stores are auto-migrated and duplicate hyperedges are collapsed.

3. Incremental extraction:
- Structure/document/prompt extractors now use checkpoint skip logic.
- Graph extraction scopes work to files changed since `graph_last_sha`.

4. MCP transport:
- Stdio is the only supported transport.
- Legacy `mcp.transport = "sse"` is normalized to stdio for compatibility.

## Upgrade Guidance

1. Update binaries and run `homer update` once to apply store migrations.
2. Re-run your normal workflow (`homer status`, `homer query`, MCP usage) and
   verify expected outputs.
3. If you have MCP config with `transport = "sse"`, change it to `stdio`
   (legacy alias still works, but stdio is canonical).

## Fallback Guidance

If you need to roll back quickly:

1. Keep a backup of `.homer/homer.db` before first run on the new version.
2. If rollback is required, restore the backup DB and previous binary together.
3. Re-run `homer update --force` after re-upgrading to rebuild cleanly.

## Compatibility Window

Legacy role/key aliases remain in read paths until sunset criteria in
`docs/compatibility.md` are met (migration maturity, tests, and documented
release notes).
