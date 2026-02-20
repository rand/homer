# ADR 0002: Deterministic Hyperedge Identity for Idempotent Upserts

## Status
Accepted

## Date
2026-02-20

## Context
Repeated extraction runs could insert semantically equivalent hyperedges multiple
times because `upsert_hyperedge` always performed insert-only writes.
This caused duplicate growth and undermined incremental guarantees.

## Decision
Introduce a deterministic `identity_key` for each hyperedge and upsert on that
identity.

Identity key definition:
- Prefix: `HyperedgeKind`
- Members: sorted tuples of `(role, node_id)`
- Canonical format: `"{kind}|{role}:{node_id}|..."`

`position` is intentionally excluded from identity so equivalent relationships
with different insertion order remain idempotent.

Schema changes:
- Add `hyperedges.identity_key`
- Add unique index `uq_hyperedges_identity_key`
- Update store write path to `INSERT ... ON CONFLICT(identity_key) DO UPDATE`

## Collision Analysis
- The identity key is deterministic and exact for `(kind, role, node_id)` sets.
- Collisions would require distinct edges to share the same kind and identical
  role/member set; those are treated as semantically equivalent by design.
- This intentionally collapses duplicate writes rather than preserving
  extraction-event multiplicity.

## Migration Plan
- On store initialization:
1. Add `identity_key` column when missing.
2. Backfill identity keys for existing rows from current members.
3. Deduplicate by identity (keep highest `id` per identity).
4. Enforce unique index.

## Consequences
- Hyperedge writes are idempotent across repeated runs and forced re-extraction.
- Legacy databases are migrated in-place without manual intervention.
- Duplicate growth regressions become testable via store and pipeline tests.
