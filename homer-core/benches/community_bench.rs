// Benchmark Louvain community detection at varying graph sizes.

use std::collections::HashMap;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use petgraph::graph::{DiGraph, NodeIndex};

use homer_core::analyze::community::louvain_full;
use homer_core::types::{InMemoryGraph, NodeId};

/// Build a synthetic graph with community structure for Louvain benchmarking.
///
/// Creates `num_communities` clusters of `cluster_size` nodes each.
/// Intra-community edges are dense; inter-community edges are sparse.
fn build_clustered_graph(num_communities: usize, cluster_size: usize) -> InMemoryGraph {
    let n = num_communities * cluster_size;
    let mut graph = DiGraph::<NodeId, f64>::with_capacity(n, n * 4);
    let mut node_to_index = HashMap::with_capacity(n);
    let mut index_to_node = HashMap::with_capacity(n);

    for i in 0..n {
        let nid = NodeId(i64::try_from(i).expect("fits"));
        let idx = graph.add_node(nid);
        node_to_index.insert(nid, idx);
        index_to_node.insert(idx, nid);
    }

    // Dense intra-community edges: connect each node to ~3 others in its cluster
    let primes = [3, 7, 11];
    for c in 0..num_communities {
        let base = c * cluster_size;
        for &p in &primes {
            for i in 0..cluster_size {
                let src = base + i;
                let tgt = base + ((i * p + 1) % cluster_size);
                if src != tgt {
                    let si = NodeIndex::new(src);
                    let ti = NodeIndex::new(tgt);
                    graph.add_edge(si, ti, 1.0);
                }
            }
        }
    }

    // Sparse inter-community edges: ~1 per 10 nodes connects to neighboring community
    for c in 0..num_communities.saturating_sub(1) {
        let base = c * cluster_size;
        let next_base = (c + 1) * cluster_size;
        for i in (0..cluster_size).step_by(10) {
            let src = NodeIndex::new(base + i);
            let tgt = NodeIndex::new(next_base + (i % cluster_size));
            graph.add_edge(src, tgt, 0.2);
        }
    }

    InMemoryGraph {
        graph,
        node_to_index,
        index_to_node,
    }
}

fn bench_louvain(c: &mut Criterion) {
    let mut group = c.benchmark_group("louvain");

    // Small: 10 communities x 100 nodes = 1,000 nodes
    // Medium: 20 communities x 500 nodes = 10,000 nodes
    // Large: 50 communities x 500 nodes = 25,000 nodes
    for (communities, cluster_size) in [(10, 100), (20, 500), (50, 500)] {
        let graph = build_clustered_graph(communities, cluster_size);
        let total = communities * cluster_size;

        group.bench_with_input(BenchmarkId::new("nodes", total), &graph, |b, g| {
            b.iter(|| {
                louvain_full(g);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_louvain);
criterion_main!(benches);
