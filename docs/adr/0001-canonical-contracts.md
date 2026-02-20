# ADR 0001: Canonical Edge Roles and Analysis Keys

## Status
Accepted

## Date
2026-02-20

## Context
Multiple components consumed shared graph edges and analysis payloads using
inconsistent role names and key names. This caused silent data loss in some
analysis and rendering paths and made behavior depend on historical data shape.

## Decision
Define one canonical contract for roles and keys and centralize it in
`homer-core/src/contracts.rs`.

### Canonical edge roles
- `Calls`: `caller`, `callee`
- `Imports`: `importer`, `imported`
- `Documents`: `document`, `code_entity`
- `BelongsTo`: `member`, `container`

### Canonical analysis keys
- `CompositeSalience`: `score`
- `PageRank`: `pagerank`
- `BetweennessCentrality`: `betweenness`
- `HITSScore`: `authority_score`
- `ContributorConcentration`: `bus_factor`, `top_contributor_share`

### Canonical metadata keys
- `Document`: `doc_type`

### Units
- Co-change confidence is stored as a fraction in `[0, 1]` and rendered as a
  percentage via explicit conversion.

## Compatibility policy
Read-path compatibility is preserved for legacy values:
- Imports: `source`/`target` are accepted as legacy aliases.
- Documents: `entity`/`subject` are accepted as legacy aliases.
- Contributor concentration: `top_contributor_pct` is accepted as a legacy key.
- Document metadata: `type` is accepted as a legacy alias for `doc_type`.

Writers now emit canonical names only.

Sunset criteria for removing these aliases are defined in
`docs/compatibility.md`.

## Consequences
- Cross-component behavior is deterministic and testable.
- Backward compatibility is preserved during transition.
- New code should use constants and helpers from `contracts.rs` instead of raw
  string literals.
