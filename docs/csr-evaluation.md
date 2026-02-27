# CSR Graph Representation Evaluation

**Task**: homer-3b1 | **Date**: 2026-02-26 | **Verdict**: NOT RECOMMENDED

## Summary

Evaluated replacing `petgraph::graph::DiGraph` with `petgraph::csr::Csr` (Compressed Sparse
Row) for read-only analysis passes. CSR is not beneficial due to existing Vec-based optimizations,
API limitations, and minimal expected gains at Homer's graph sizes.

## Analysis by Algorithm

### Louvain Community Detection (community.rs)

Already uses `AdjList = Vec<Vec<(usize, f64)>>` — a Vec-indexed adjacency list built from
DiGraph at the start of `louvain_full()`. This is functionally equivalent to CSR with variable-
length rows: contiguous node indices, per-node neighbor vectors. The Louvain optimization in
commit a3d555e achieved **625x speedup** precisely by switching from HashMap to Vec-based
adjacency. CSR (flat array + offset array) would save one level of indirection per neighbor
lookup but at Homer's graph sizes (1K-100K nodes), this is within noise.

**Graph contraction** in Phase 2 rebuilds the adjacency list each level. CSR requires sorted,
finalized construction — the dynamic contraction step would need materialization into a new CSR
each level, adding conversion overhead.

### Brandes Betweenness (centrality.rs)

Uses `graph.neighbors(v)` in BFS — DiGraph's adjacency iteration is already contiguous per node.
The dominant cost is the BFS itself, now parallelized with rayon (each source BFS is independent).
At 10K nodes, the bottleneck is the O(V*E) algorithm complexity, not the memory access pattern
of neighbor iteration. CSR neighbor iteration would save ~1 pointer dereference per edge traversal
— negligible vs. the algorithmic cost.

### PageRank (centrality.rs)

Uses `petgraph::algo::page_rank()`, a library function that operates on types implementing the
`NodeIndexable + IntoEdges + IntoNodeIdentifiers + GraphProp + NodeCount` trait bundle.
`petgraph::csr::Csr` implements these traits, so PageRank would work on CSR. However, PageRank
converges in 15-30 iterations at typical graph sizes, each iterating all edges once — the
same O(E) per iteration as DiGraph. No measurable difference expected.

### HITS (centrality.rs)

Uses `graph.edge_indices()` + `graph.edge_endpoints()` in a single-pass formulation. CSR
supports edge iteration via `edge_references()`. The iteration pattern is identical: one pass
over all edges per iteration. No benefit from CSR.

## API Limitations

`petgraph::csr::Csr` has significant limitations vs `DiGraph`:

1. **No edge modification** after construction — acceptable for read-only analysis but requires
   a conversion step from DiGraph (which is built dynamically in `InMemoryGraph::from_edges`)
2. **No edge weight access by EdgeIndex** — Csr doesn't support `graph[edge_idx]` indexing,
   complicating Louvain weight access
3. **No `edge_endpoints(EdgeIndex)`** — breaks HITS iteration pattern
4. **Petgraph's algorithm library** (page_rank, dijkstra, etc.) works on DiGraph; some
   algorithms may not be available for Csr due to missing trait impls

## Conversion Overhead

Converting DiGraph → CSR requires:
1. Collecting all edges as (src, tgt, weight) triples
2. Sorting by source node
3. Building the CSR offset + edge arrays

For a graph with 100K edges, this is ~O(E log E) for the sort. The analysis algorithms then
iterate edges O(iterations * E). At 100 iterations, conversion is ~1% overhead — acceptable.
But the API limitations above make this moot.

## Recommendation

**Do not adopt CSR.** The existing optimizations already address cache-friendliness:

- Louvain: Vec-based AdjList (equivalent to CSR in practice)
- Brandes: rayon parallelism dominates; neighbor iteration is not the bottleneck
- PageRank/HITS: O(E) per iteration regardless of representation

If future profiling reveals neighbor iteration as a bottleneck (>10% of total time), consider:
1. A flat CSR-like struct with `offsets: Vec<usize>` + `targets: Vec<(usize, f64)>` built
   from the existing AdjList, avoiding petgraph's Csr API limitations
2. `graph` crate (faster traversal) or `rustworkx` (Python-oriented, not applicable)

The current `DiGraph` + `Vec<Vec<(usize, f64)>>` combination is the right tradeoff for Homer's
graph sizes and access patterns.
