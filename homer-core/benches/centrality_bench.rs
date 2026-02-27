// Benchmark centrality algorithms: PageRank, Betweenness, HITS at varying graph sizes.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use petgraph::graph::DiGraph;

use homer_core::analyze::centrality::{brandes_betweenness, hits_power_iteration};
use homer_core::types::NodeId;

/// Build a synthetic directed graph that mimics a call/import graph.
///
/// Structure: `node_count` nodes, ~`edge_factor` edges per node on average.
/// Edges connect node `i` to `(i * prime + offset) % node_count` for several primes,
/// producing a sparse, connected-ish graph without self-loops.
fn build_synthetic_graph(node_count: usize, edge_factor: usize) -> DiGraph<NodeId, f64> {
    let mut graph = DiGraph::<NodeId, f64>::with_capacity(node_count, node_count * edge_factor);

    for i in 0..node_count {
        graph.add_node(NodeId(i64::try_from(i).expect("node count fits in i64")));
    }

    // Use several prime multipliers to create edges
    let primes = [7, 13, 31, 61, 127, 251];
    for &prime in &primes[..edge_factor.min(primes.len())] {
        for i in 0..node_count {
            let target = (i.wrapping_mul(prime).wrapping_add(1)) % node_count;
            if target != i {
                let src = petgraph::graph::NodeIndex::new(i);
                let tgt = petgraph::graph::NodeIndex::new(target);
                graph.add_edge(src, tgt, 1.0);
            }
        }
    }

    graph
}

fn bench_pagerank(c: &mut Criterion) {
    let mut group = c.benchmark_group("pagerank");

    for node_count in [1_000, 10_000, 100_000] {
        let graph = build_synthetic_graph(node_count, 3);

        group.bench_with_input(BenchmarkId::new("nodes", node_count), &graph, |b, g| {
            b.iter(|| {
                petgraph::algo::page_rank(g, 0.85, 100);
            });
        });
    }

    group.finish();
}

fn bench_betweenness(c: &mut Criterion) {
    let mut group = c.benchmark_group("betweenness");
    // Betweenness is O(V*E), so keep sizes smaller
    group.sample_size(10);

    for node_count in [100, 500, 1_000] {
        let graph = build_synthetic_graph(node_count, 3);

        group.bench_with_input(
            BenchmarkId::new("exact_nodes", node_count),
            &graph,
            |b, g| {
                b.iter(|| {
                    brandes_betweenness(g, g.node_count());
                });
            },
        );
    }

    // Approximate betweenness at larger scales
    for node_count in [5_000, 10_000] {
        let graph = build_synthetic_graph(node_count, 3);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let k = (node_count as f64).sqrt() as usize;

        group.bench_with_input(
            BenchmarkId::new("approx_nodes", node_count),
            &(graph, k),
            |b, (g, k)| {
                b.iter(|| {
                    brandes_betweenness(g, *k);
                });
            },
        );
    }

    group.finish();
}

fn bench_hits(c: &mut Criterion) {
    let mut group = c.benchmark_group("hits");

    for node_count in [1_000, 10_000, 100_000] {
        let graph = build_synthetic_graph(node_count, 3);

        group.bench_with_input(BenchmarkId::new("nodes", node_count), &graph, |b, g| {
            b.iter(|| {
                hits_power_iteration(g, 100);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_pagerank, bench_betweenness, bench_hits);
criterion_main!(benches);
