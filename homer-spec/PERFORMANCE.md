# Homer Performance Architecture

> Parallelism, memory management, caching, and benchmarking strategy.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [STORE.md](STORE.md) · [GRAPH_ENGINE.md](GRAPH_ENGINE.md)

---

## Performance Targets

| Scenario | Target | Metric |
|----------|--------|--------|
| Initial run, 1K files, 5K commits | < 60 seconds (no LLM) | Wall-clock time |
| Initial run, 10K files, 50K commits | < 10 minutes (no LLM) | Wall-clock time |
| Incremental update, 10 new commits | < 5 seconds (no LLM) | Wall-clock time |
| LLM summarization, 50 entities | < 2 minutes | Depends on API latency |
| Rendering, all formats | < 5 seconds | Wall-clock time |
| Memory, 10K file repo | < 500 MB peak | Resident memory |
| Database size, 10K file repo | < 50 MB | File size |

These are aspirational targets for initial implementation. Profiling will reveal actual bottlenecks.

---

## Parallelism Model

### CPU-Bound Work: Rayon

Rayon provides data-parallel execution on a thread pool sized to CPU cores.

```rust
use rayon::prelude::*;

// File parsing — embarrassingly parallel
let parse_results: Vec<Result<ParsedFile>> = source_files
    .par_iter()
    .map(|file| {
        let source = std::fs::read_to_string(&file.path)?;
        let tree = parser_for_language(&file.language).parse(&source)?;
        Ok(ParsedFile { path: file.path.clone(), tree, source })
    })
    .collect();

// Scope graph construction — parallel per file
let subgraphs: Vec<Result<FileGraph>> = parsed_files
    .par_iter()
    .map(|pf| {
        let lang = registry.for_file(&pf.path)?;
        lang.build_scope_graph(&pf.source, &pf.path, &globals)
    })
    .collect();
```

**Key parallel opportunities**:
- File parsing (tree-sitter): each file is independent
- Scope graph construction: each file produces isolated subgraph
- Doc comment extraction: piggybacks on file parsing, no additional pass
- Document parsing: each document is independent
- Behavioral metrics per-file: change frequency, churn per file are independent
- Rendering: each renderer is independent

**Sequential bottlenecks**:
- Git history walking (libgit2 repo handle is not thread-safe for a single repo)
- Graph merging (subgraphs merge into shared structure)
- Global centrality metrics (PageRank needs the full graph)
- Community detection (iterative algorithm)
- Prompt extraction (source-specific parsing, low volume, sequential per source)
- SQLite writes (single-writer)

### I/O-Bound Work: Tokio

Async runtime for network operations.

```rust
// GitHub API — concurrent paginated requests
let pr_pages = futures::stream::iter(1..=total_pages)
    .map(|page| {
        let client = client.clone();
        async move {
            client.pulls(&owner, &repo)
                .page(page)
                .per_page(100)
                .send()
                .await
        }
    })
    .buffer_unordered(5)  // 5 concurrent requests
    .collect::<Vec<_>>()
    .await;

// LLM API — batched concurrent requests
let summaries = futures::stream::iter(high_salience_entities)
    .map(|entity| {
        let client = llm_client.clone();
        async move {
            let prompt = build_summary_prompt(&entity);
            client.complete(&prompt).await
        }
    })
    .buffer_unordered(config.llm.max_concurrent)
    .collect::<Vec<_>>()
    .await;
```

### Hybrid: Rayon + Tokio

When CPU work needs to trigger async I/O (e.g., analysis identifies entities needing LLM summarization):

```rust
// Run CPU analysis on Rayon, collect entities needing LLM
let entities_for_llm: Vec<Entity> = rayon::scope(|s| {
    // ... parallel analysis ...
    collect_high_salience_entities()
});

// Then run async LLM calls on Tokio
let summaries = tokio::runtime::Handle::current()
    .block_on(async {
        summarize_entities(&llm_client, &entities_for_llm).await
    });
```

---

## Memory Management

### Arena Allocation for Graph Analysis

During centrality computation, Homer loads the call graph into memory. For large repos (100K+ edges), individual `Box<Node>` allocations create fragmentation and GC pressure.

```rust
use typed_arena::Arena;

pub struct AnalysisArena {
    nodes: Arena<GraphNode>,
    edges: Arena<GraphEdge>,
}

impl AnalysisArena {
    /// Pre-allocate based on estimated graph size
    pub fn with_capacity(est_nodes: usize, est_edges: usize) -> Self {
        Self {
            nodes: Arena::with_capacity(est_nodes),
            edges: Arena::with_capacity(est_edges),
        }
    }
}

// Usage: arena is dropped after analysis pass completes,
// freeing all allocations at once
fn compute_centrality(store: &dyn HomerStore) -> Result<CentralityResults> {
    let arena = AnalysisArena::with_capacity(10_000, 50_000);
    let graph = load_graph_into_arena(store, &arena)?;
    let results = run_pagerank(&graph)?;
    // arena dropped here — all graph memory freed instantly
    Ok(results)
}
```

### String Interning

Function names, file paths, and type names repeat extensively in the graph. Intern them:

```rust
use string_interner::{StringInterner, DefaultSymbol};

pub struct InternedNames {
    interner: StringInterner,
}

impl InternedNames {
    pub fn intern(&mut self, name: &str) -> DefaultSymbol {
        self.interner.get_or_intern(name)
    }
    
    pub fn resolve(&self, sym: DefaultSymbol) -> &str {
        self.interner.resolve(sym).unwrap()
    }
}
```

This reduces memory for repeated strings from O(n × avg_len) to O(unique × avg_len).

### Lazy Loading

Not every operation needs the full graph:

```rust
// homer query src/auth/validate.rs — only needs 1-hop neighborhood
let subgraph = store.load_call_graph(&SubgraphFilter::Neighborhood {
    centers: vec![node_id],
    hops: 1,
}).await?;

// homer graph --metric pagerank — needs full call graph (unavoidable)
let full_graph = store.load_call_graph(&SubgraphFilter::Full).await?;

// homer render --format module-ctx src/auth/ — only needs one module
let module_graph = store.load_call_graph(&SubgraphFilter::Module {
    path_prefix: "src/auth/".into(),
}).await?;
```

---

## SQLite Performance

### Transaction Batching

Bulk inserts without transactions: ~50 inserts/sec (each is a separate fsync).  
Bulk inserts in transactions of 1000: ~100,000 inserts/sec.

```rust
impl SqliteStore {
    pub async fn bulk_insert_nodes(&self, nodes: &[Node]) -> Result<Vec<NodeId>> {
        let mut ids = Vec::with_capacity(nodes.len());
        
        for chunk in nodes.chunks(BATCH_SIZE) {  // BATCH_SIZE = 1000
            let chunk_ids = self.conn.call(move |conn| {
                let tx = conn.transaction()?;
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO nodes (kind, name, content_hash, last_extracted, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5)"
                )?;
                
                let mut batch_ids = Vec::new();
                for node in chunk {
                    stmt.execute(params![
                        node.kind.as_str(),
                        &node.name,
                        node.content_hash,
                        node.last_extracted.to_rfc3339(),
                        serde_json::to_string(&node.metadata)?,
                    ])?;
                    batch_ids.push(NodeId(conn.last_insert_rowid()));
                }
                tx.commit()?;
                Ok(batch_ids)
            }).await?;
            
            ids.extend(chunk_ids);
        }
        
        Ok(ids)
    }
}
```

### Prepared Statement Reuse

Cache frequently used prepared statements:

```rust
// The rusqlite `prepare_cached` method handles this automatically.
// Ensure all hot-path queries use prepare_cached, not prepare.
let mut stmt = conn.prepare_cached("SELECT * FROM nodes WHERE kind = ?1 AND name = ?2")?;
```

### WAL Mode

Write-Ahead Logging enables concurrent reads during writes:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;  -- Safe with WAL, faster than FULL
PRAGMA cache_size = -64000;   -- 64MB page cache
PRAGMA mmap_size = 268435456; -- 256MB memory-mapped I/O
```

Set these on database open.

### Index Strategy

Indexes defined in [STORE.md](STORE.md) schema. Key principle: index columns used in WHERE clauses and JOINs, but don't over-index (each index slows writes).

**Hot query patterns to optimize**:
- `nodes WHERE kind = ? AND name = ?` (entity lookup) — covered by UNIQUE index
- `hyperedge_members WHERE node_id = ?` (find edges involving a node) — indexed
- `analysis_results WHERE node_id = ? AND kind = ?` (get analysis for entity) — covered by UNIQUE index
- `text_search MATCH ?` (full-text search) — FTS5 handles this

---

## Incremental Computation

### Salsa-Inspired Memoization

The key insight from the [Salsa](https://salsa-rs.github.io/salsa) framework (used in Topos and rust-analyzer): define computations as pure functions of their inputs, and memoize results. When inputs change, only recompute affected downstream functions.

Homer's computation DAG:

```
file_content(file_id)
    → ast(file_id)
        → scope_graph(file_id)
            → call_graph(module_id)
                → pagerank(graph_snapshot)
                → betweenness(graph_snapshot)
                → community_assignment(graph_snapshot)
    → behavioral_metrics(file_id)
        → change_frequency(file_id)
        → co_change_sets()
    → composite_salience(node_id)
        → semantic_summary(node_id)  [only if salience > threshold]
```

**Implementation approach**: Rather than depending on Salsa directly (which is designed for IDE-style incremental computation with fine-grained tracking), implement a simpler `input_hash`-based memoization:

```rust
/// Check if a computation needs to rerun
fn needs_recompute(
    store: &dyn HomerStore,
    node_id: NodeId,
    analysis_kind: AnalysisKind,
    current_input_hash: u64,
) -> Result<bool> {
    match store.get_analysis(node_id, analysis_kind).await? {
        Some(existing) => Ok(existing.input_hash != current_input_hash),
        None => Ok(true),  // Never computed
    }
}

/// Compute hash of inputs for a given analysis
fn compute_input_hash(inputs: &AnalysisInputs) -> u64 {
    let mut hasher = XxHash64::default();
    // Hash all input data that affects this computation
    inputs.hash(&mut hasher);
    hasher.finish()
}
```

This is coarser than Salsa's fine-grained dependency tracking but much simpler to implement and sufficient for Homer's batch-oriented workload.

---

## Benchmarking Infrastructure

### Criterion Benchmarks

```rust
// benches/parse_throughput.rs
fn bench_parse_large_repo(c: &mut Criterion) {
    let files = load_fixture_files("fixtures/large-repo");
    
    c.bench_function("parse_1000_files", |b| {
        b.iter(|| {
            files.par_iter()
                .map(|f| parse_file(f))
                .collect::<Vec<_>>()
        })
    });
}

// benches/centrality.rs
fn bench_pagerank(c: &mut Criterion) {
    let mut group = c.benchmark_group("pagerank");
    
    for size in [1_000, 10_000, 100_000] {
        let graph = generate_random_graph(size, size * 5);
        group.bench_with_input(
            BenchmarkId::new("nodes", size),
            &graph,
            |b, g| b.iter(|| compute_pagerank(g)),
        );
    }
    group.finish();
}

// benches/sqlite_insert.rs
fn bench_bulk_insert(c: &mut Criterion) {
    let nodes = generate_test_nodes(10_000);
    
    c.bench_function("insert_10k_nodes", |b| {
        b.iter(|| {
            let store = SqliteStore::in_memory().unwrap();
            store.bulk_insert_nodes(&nodes).unwrap()
        })
    });
}

// benches/incremental_update.rs
fn bench_incremental(c: &mut Criterion) {
    // Pre-populate store with 10K nodes
    let store = setup_populated_store(10_000);
    let new_commits = generate_commits(10);
    
    c.bench_function("update_10_commits", |b| {
        b.iter(|| {
            process_incremental_update(&store, &new_commits)
        })
    });
}
```

### Profiling Hooks

Optional `tracing` instrumentation for detailed profiling:

```rust
use tracing::{instrument, info_span};

#[instrument(skip(store))]
async fn extract_git_history(store: &dyn HomerStore, config: &ExtractConfig) -> Result<ExtractStats> {
    let _span = info_span!("git_extraction").entered();
    // ...
}
```

Enable with `HOMER_LOG=homer=trace` for full span timing, or `HOMER_LOG=homer=info` for stage-level timing.

### Progress Reporting

For long operations, report progress:

```rust
pub trait ProgressReporter: Send + Sync {
    fn start(&self, task: &str, total: Option<u64>);
    fn advance(&self, amount: u64);
    fn finish(&self);
    fn message(&self, msg: &str);
}
```

CLI uses `indicatif` crate for progress bars. Library callers can provide their own reporter or use `NoopReporter`.
