# Compatibility Policy

## Scope
This policy covers cross-component contract compatibility for:
- hyperedge member roles
- analysis result keys
- metadata keys consumed by analyzers/renderers/MCP tools

## Canonical Contract
Canonical names are defined in `homer-core/src/contracts.rs` and are the only
names writers should emit.

## Read Compatibility Window
Legacy read aliases remain supported during the current migration window:
- Imports: `source`/`target` (legacy) map to `importer`/`imported`.
- Documents: `entity`/`subject` (legacy) map to `code_entity`.
- Contributor concentration: `top_contributor_pct` (legacy) maps to
  `top_contributor_share`.
- Document metadata: `type` (legacy) maps to `doc_type` where needed.

## Sunset Criteria
Legacy aliases may be removed only when all of the following are true:
1. `homer-core` schema has advanced by at least one major migration from the
   canonical-contract baseline (schema version `2`).
2. Contract regression tests pass with legacy fixtures removed.
3. Release notes include explicit migration guidance and fallback steps.

Until those criteria are met, compatibility read paths are required and treated
as part of the public behavior contract.
