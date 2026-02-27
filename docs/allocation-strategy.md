# Arena Allocation Strategy Evaluation

**Task**: homer-r0c | **Date**: 2026-02-26 | **Verdict**: NOT RECOMMENDED (arenas); RECOMMEND string interning instead

## Summary

Evaluated bumpalo, typed-arena, and alternative allocation strategies for tree-sitter scope
graph construction and resolution. Arenas are the wrong tool for this workload due to !Send
constraints (rayon parallelism), data lifetime requirements, and the actual allocation pressure
being in resolution BFS rather than construction. String interning is recommended as a more
targeted optimization.

## Current Allocation Profile

### Scope Graph Construction (extract/graph.rs)

File parsing is parallelized with rayon (`par_iter` over eligible files). Each file produces:
- `FileScopeGraph` containing `Vec<ScopeNode>` + `Vec<ScopeEdge>`
- `ScopeNode` is ~80 bytes: `ScopeNodeId(u32)`, `ScopeNodeKind` (enum with heap `String` for
  symbol names), `PathBuf`, `Option<TextRange>`, `Option<SymbolKind>`
- `ScopeEdge` is 12 bytes: two `ScopeNodeId(u32)` + `u8` precedence

**Allocation pattern**: Per-file Vec allocations are efficient — Vec grows geometrically,
amortizing allocation cost. The main waste is in symbol name duplication: `PushSymbol { symbol: "foo" }`
and `PopSymbol { symbol: "foo" }` allocate separate heap Strings for the same identifier.

After parsing, all `FileScopeGraph` data is merged into a combined `ScopeGraph` via
`add_file_graph()`, which remaps IDs and inserts into `HashMap<ScopeNodeId, ScopeNode>`.
Data must outlive the parallel parse phase.

### Scope Graph Resolution (scope_graph.rs resolve_all)

**This is the hot allocation path.** For each PushSymbol reference, BFS resolution creates
`PartialPath` structs, each containing:
- `symbol_stack: Vec<String>` — cloned at each BFS step
- `visited: HashSet<ScopeNodeId>` — cloned at each BFS step
- `current_node: ScopeNodeId` (copy)
- `start_node: ScopeNodeId` (copy)

For a file with 1000 references, each requiring ~10 BFS steps, this produces ~10,000
PartialPath allocations with Vec and HashSet clones. This dwarfs construction costs.

### Enclosing Function Computation (call_graph.rs compute_enclosing_functions)

O(references * functions) per file — linear scan of all functions for each reference to find
smallest containing span. Not allocation-heavy, but algorithmically suboptimal. Binary search
on sorted spans would reduce to O((refs + funcs) * log(funcs)).

## Arena Evaluation

### bumpalo

- **!Send**: Cannot be shared across rayon threads. Workaround: per-thread arenas via
  `thread_local!` or rayon's `ThreadLocal<RefCell<Bump>>`.
- **Lifetime problem**: `FileScopeGraph` data must outlive the parallel block (merged into
  global `ScopeGraph`). Arena-allocated data is freed when the arena drops. Either:
  - Arena must outlive the entire extraction phase (defeating the purpose of per-file arenas)
  - Data must be copied out of the arena before it drops (negating the allocation benefit)
- **String storage**: bumpalo can allocate strings efficiently, but the ScopeNodeKind enum
  owns its strings — would need invasive type changes to use `&'arena str`.
- **Verdict**: Poor fit. The ownership model conflicts with data flow.

### typed-arena

- Also !Send — same rayon issues as bumpalo
- Single-type allocation: `Arena<ScopeNode>` for nodes, separate `Arena<ScopeEdge>` for edges
- Same lifetime problems as bumpalo
- **Verdict**: Poor fit for the same reasons.

### jemalloc with size classes

- Drop-in allocator replacement: `#[global_allocator] static GLOBAL: jemallocator::Jemalloc`
- Reduces fragmentation for workloads with many same-sized allocations
- No code changes needed, but benefits are typically 5-10% for allocation-heavy workloads
- **Verdict**: Low-effort, modest gain. Worth trying as a global allocator but orthogonal
  to the scope graph allocation pattern.

## Better Alternatives

### 1. String Interning (RECOMMENDED)

Replace heap-allocated symbol names with interned keys:

```rust
// Before: each PushSymbol/PopSymbol owns a String
PushSymbol { symbol: String }

// After: shared string table, nodes store cheap keys
PushSymbol { symbol: SymbolKey }  // SymbolKey is Copy, typically u32
```

Libraries: `lasso` (concurrent, ~2x faster than `string-interner`), `string-interner` (simpler).

Benefits:
- Eliminates duplicate String allocations (same symbol referenced many times)
- `SymbolKey` is Copy — no clone cost in PartialPath symbol_stack
- Comparison is integer equality instead of string comparison
- Estimated 30-50% reduction in heap allocations during scope graph construction

Trade-off: Requires a new `StringInterner` passed through the scope graph pipeline. Moderate
refactor (~200 lines across scope_graph.rs, language implementations).

### 2. Resolution Cache (homer-1ml)

Cache resolved references by (starting_scope, symbol_name):
```rust
cache: HashMap<(ScopeNodeId, String), Vec<ResolvedReference>>
```

If 1000 references to 50 unique symbols exist, the cache turns 1000 BFS traversals into 50.
This eliminates the PartialPath allocation pressure entirely for cache hits.

### 3. Binary Search for Enclosing Functions (homer-1ml)

Sort function spans by start position, then binary search for each reference span.
Reduces O(refs * funcs) to O((refs + funcs) * log(funcs)). Simple, high-impact for files
with many functions.

### 4. SoA Layout (soa_derive)

Transform `Vec<ScopeNode>` (AoS) to struct-of-arrays:
```rust
struct ScopeNodes {
    ids: Vec<ScopeNodeId>,
    kinds: Vec<ScopeNodeKind>,
    file_paths: Vec<PathBuf>,
    spans: Vec<Option<TextRange>>,
    symbol_kinds: Vec<Option<SymbolKind>>,
}
```

Improves cache locality when iterating only over spans (enclosing function) or only over kinds
(collecting push nodes). However, ScopeGraph uses HashMap<ScopeNodeId, ScopeNode> for random
access, which negates SoA benefits. Only useful if ScopeGraph is restructured to Vec-indexed
storage.

## Recommendations

1. **Skip arena allocation** — wrong tool for this workload
2. **Implement resolution cache** (homer-1ml) — highest impact, addresses the real bottleneck
3. **Implement binary search for enclosing functions** (homer-1ml) — simple algorithmic win
4. **Consider string interning** as a future optimization if profiling shows symbol name
   allocation as significant after caching
5. **SmallVec for HyperedgeMember** (homer-5bm) — independent of arena research, proceed
6. **mmap for file reading** (homer-he8) — independent of arena research, proceed
