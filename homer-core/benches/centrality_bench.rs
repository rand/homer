// Benchmark centrality algorithms: PageRank, Betweenness, HITS at varying graph sizes.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use petgraph::graph::DiGraph;

use homer_core::types::NodeId;

/// Build a synthetic directed graph that mimics a call/import graph.
///
/// Structure: `node_count` nodes, ~`edge_factor` edges per node on average.
/// Edges connect node `i` to `(i * prime + offset) % node_count` for several primes,
/// producing a sparse, connected-ish graph without self-loops.
fn build_synthetic_graph(node_count: usize, edge_factor: usize) -> DiGraph<NodeId, f64> {
    let mut graph = DiGraph::<NodeId, f64>::with_capacity(node_count, node_count * edge_factor);

    for i in 0..node_count {
        graph.add_node(NodeId(i as i64));
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

        group.bench_with_input(
            BenchmarkId::new("nodes", node_count),
            &graph,
            |b, g| {
                b.iter(|| {
                    petgraph::algo::page_rank(g, 0.85, 100);
                });
            },
        );
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

        group.bench_with_input(
            BenchmarkId::new("nodes", node_count),
            &graph,
            |b, g| {
                b.iter(|| {
                    hits_power_iteration(g, 100);
                });
            },
        );
    }

    group.finish();
}

// ── Copied algorithm implementations from centrality.rs ──────────────
// Benchmarks need direct access to the algorithm functions which aren't
// publicly exported. We inline the core algorithms here to avoid
// exposing internal implementation details just for benchmarking.

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
fn brandes_betweenness(graph: &DiGraph<NodeId, f64>, k: usize) -> Vec<f64> {
    let n = graph.node_count();
    if n == 0 {
        return vec![];
    }

    let mut cb = vec![0.0_f64; n];

    let sources: Vec<petgraph::graph::NodeIndex> = if k >= n {
        graph.node_indices().collect()
    } else {
        let step = n / k;
        graph
            .node_indices()
            .step_by(step.max(1))
            .take(k)
            .collect()
    };

    for &s in &sources {
        let s_idx = s.index();
        let mut stack: Vec<petgraph::graph::NodeIndex> = Vec::new();
        let mut predecessors: Vec<Vec<petgraph::graph::NodeIndex>> = vec![vec![]; n];
        let mut sigma = vec![0.0_f64; n];
        sigma[s_idx] = 1.0;
        let mut dist: Vec<i64> = vec![-1; n];
        dist[s_idx] = 0;

        let mut queue = std::collections::VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            let v_idx = v.index();

            for neighbor in graph.neighbors(v) {
                let w_idx = neighbor.index();
                if dist[w_idx] < 0 {
                    dist[w_idx] = dist[v_idx] + 1;
                    queue.push_back(neighbor);
                }
                if dist[w_idx] == dist[v_idx] + 1 {
                    sigma[w_idx] += sigma[v_idx];
                    predecessors[w_idx].push(v);
                }
            }
        }

        let mut delta = vec![0.0_f64; n];
        while let Some(w) = stack.pop() {
            let w_idx = w.index();
            for &v in &predecessors[w_idx] {
                let v_idx = v.index();
                let ratio = sigma[v_idx] / sigma[w_idx];
                delta[v_idx] += ratio * (1.0 + delta[w_idx]);
            }
            if w != s {
                cb[w_idx] += delta[w_idx];
            }
        }
    }

    let scale = if k < n {
        n as f64 / k as f64
    } else {
        1.0
    };

    let max_cb = cb.iter().copied().fold(0.0_f64, f64::max);
    if max_cb > 0.0 {
        cb.iter().map(|&v| (v * scale) / (max_cb * scale)).collect()
    } else {
        cb
    }
}

#[allow(clippy::cast_precision_loss)]
fn hits_power_iteration(
    graph: &DiGraph<NodeId, f64>,
    max_iter: usize,
) -> (Vec<f64>, Vec<f64>) {
    let n = graph.node_count();
    if n == 0 {
        return (vec![], vec![]);
    }

    let mut hubs = vec![1.0_f64; n];
    let mut authorities = vec![1.0_f64; n];

    for _ in 0..max_iter {
        let mut new_auth = vec![0.0_f64; n];
        for edge in graph.edge_indices() {
            if let Some((src, tgt)) = graph.edge_endpoints(edge) {
                new_auth[tgt.index()] += hubs[src.index()];
            }
        }

        let mut new_hub = vec![0.0_f64; n];
        for edge in graph.edge_indices() {
            if let Some((src, tgt)) = graph.edge_endpoints(edge) {
                new_hub[src.index()] += new_auth[tgt.index()];
            }
        }

        let auth_norm = new_auth.iter().map(|x| x * x).sum::<f64>().sqrt();
        let hub_norm = new_hub.iter().map(|x| x * x).sum::<f64>().sqrt();

        if auth_norm > 0.0 {
            for a in &mut new_auth {
                *a /= auth_norm;
            }
        }
        if hub_norm > 0.0 {
            for h in &mut new_hub {
                *h /= hub_norm;
            }
        }

        let auth_diff: f64 = new_auth
            .iter()
            .zip(authorities.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        let hub_diff: f64 = new_hub
            .iter()
            .zip(hubs.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();

        authorities = new_auth;
        hubs = new_hub;

        if auth_diff < 1e-10 && hub_diff < 1e-10 {
            break;
        }
    }

    (hubs, authorities)
}

criterion_group!(benches, bench_pagerank, bench_betweenness, bench_hits);
criterion_main!(benches);
