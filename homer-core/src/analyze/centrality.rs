// Centrality analysis: PageRank, Betweenness, HITS, CompositeSalience.
//
// Graph algorithms intentionally cast int↔float (precision loss acceptable for metrics).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use petgraph::graph::{DiGraph, NodeIndex};
use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind, NodeId,
};

use super::AnalyzeStats;
use super::traits::Analyzer;

// ── Configuration ──────────────────────────────────────────────────

/// Configuration for centrality algorithms.
#[derive(Debug, Clone)]
pub struct CentralityConfig {
    /// Node count above which betweenness uses k-source approximation.
    pub approx_threshold: usize,
    /// Number of source nodes for approximate betweenness (default: sqrt(V)).
    pub approx_k: Option<usize>,
    /// `PageRank` damping factor.
    pub damping: f64,
    /// Max iterations for iterative algorithms.
    pub max_iterations: u32,
}

impl Default for CentralityConfig {
    fn default() -> Self {
        Self {
            approx_threshold: 50_000,
            approx_k: None,
            damping: 0.85,
            max_iterations: 100,
        }
    }
}

// ── In-memory graph ────────────────────────────────────────────────

/// A petgraph `DiGraph` loaded from the store, with `NodeId` ↔ `NodeIndex` mapping.
#[derive(Debug)]
pub struct InMemoryGraph {
    pub graph: DiGraph<NodeId, f64>,
    pub node_to_index: HashMap<NodeId, NodeIndex>,
    pub index_to_node: HashMap<NodeIndex, NodeId>,
}

impl InMemoryGraph {
    /// Load a directed graph from the store for the given edge kind.
    ///
    /// For `Calls` edges: caller → callee.
    /// For `Imports` edges: importer → imported.
    pub async fn from_store(
        store: &dyn HomerStore,
        edge_kind: HyperedgeKind,
    ) -> crate::error::Result<Self> {
        let edges = store.get_edges_by_kind(edge_kind).await?;

        let mut graph = DiGraph::<NodeId, f64>::new();
        let mut node_to_index: HashMap<NodeId, NodeIndex> = HashMap::new();
        let mut index_to_node: HashMap<NodeIndex, NodeId> = HashMap::new();

        // Ensure all member nodes are in the graph
        for edge in &edges {
            for member in &edge.members {
                node_to_index.entry(member.node_id).or_insert_with(|| {
                    let idx = graph.add_node(member.node_id);
                    index_to_node.insert(idx, member.node_id);
                    idx
                });
            }
        }

        // Add directed edges: role "caller"→"callee" or position 0→1
        for edge in &edges {
            let (source, target) = extract_directed_pair(&edge.members);
            if let (Some(&src_idx), Some(&tgt_idx)) =
                (node_to_index.get(&source), node_to_index.get(&target))
            {
                graph.add_edge(src_idx, tgt_idx, edge.confidence);
            }
        }

        Ok(Self {
            graph,
            node_to_index,
            index_to_node,
        })
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

/// Extract a directed (source, target) pair from hyperedge members.
/// Uses roles ("caller"/"callee", "source"/"target") or falls back to position ordering.
fn extract_directed_pair(
    members: &[crate::types::HyperedgeMember],
) -> (NodeId, NodeId) {
    if members.len() < 2 {
        // Degenerate — self-loop or single member
        let id = members.first().map_or(NodeId(0), |m| m.node_id);
        return (id, id);
    }

    // Try role-based direction
    let source_roles = ["caller", "source", "importer"];
    let target_roles = ["callee", "target", "imported"];

    let source = members
        .iter()
        .find(|m| source_roles.contains(&m.role.as_str()));
    let target = members
        .iter()
        .find(|m| target_roles.contains(&m.role.as_str()));

    if let (Some(s), Some(t)) = (source, target) {
        return (s.node_id, t.node_id);
    }

    // Fallback: position ordering
    let mut sorted = members.to_vec();
    sorted.sort_by_key(|m| m.position);
    (sorted[0].node_id, sorted[1].node_id)
}

// ── Centrality Analyzer ────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct CentralityAnalyzer {
    pub config: CentralityConfig,
}

#[async_trait::async_trait]
impl Analyzer for CentralityAnalyzer {
    fn name(&self) -> &'static str {
        "centrality"
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Load call graph for PageRank + HITS
        let call_graph = InMemoryGraph::from_store(store, HyperedgeKind::Calls).await?;
        info!(
            nodes = call_graph.node_count(),
            edges = call_graph.edge_count(),
            "Loaded call graph"
        );

        // Load import graph for betweenness
        let import_graph = InMemoryGraph::from_store(store, HyperedgeKind::Imports).await?;
        info!(
            nodes = import_graph.node_count(),
            edges = import_graph.edge_count(),
            "Loaded import graph"
        );

        if call_graph.node_count() == 0 && import_graph.node_count() == 0 {
            info!("No graph data found, skipping centrality analysis");
            stats.duration = start.elapsed();
            return Ok(stats);
        }

        // ── PageRank on call graph ──────────────────────────────────
        let pagerank_scores = compute_pagerank(&call_graph, &self.config);
        let pr_count = store_centrality_results(
            store,
            &call_graph,
            &pagerank_scores,
            AnalysisKind::PageRank,
            "pagerank",
        )
        .await?;
        stats.results_stored += pr_count;

        // ── Betweenness on import graph ─────────────────────────────
        let betweenness_scores = compute_betweenness(&import_graph, &self.config);
        let bc_count = store_centrality_results(
            store,
            &import_graph,
            &betweenness_scores,
            AnalysisKind::BetweennessCentrality,
            "betweenness",
        )
        .await?;
        stats.results_stored += bc_count;

        // ── HITS on call graph ──────────────────────────────────────
        let (hub_scores, authority_scores) = compute_hits(&call_graph, &self.config);
        let hits_count =
            store_hits_results(store, &call_graph, &hub_scores, &authority_scores).await?;
        stats.results_stored += hits_count;

        // ── Composite Salience ──────────────────────────────────────
        let salience_count = compute_and_store_salience(
            store,
            &call_graph,
            &import_graph,
            &pagerank_scores,
            &betweenness_scores,
            &authority_scores,
        )
        .await?;
        stats.results_stored += salience_count;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Centrality analysis complete"
        );

        Ok(stats)
    }
}

// ── PageRank ───────────────────────────────────────────────────────

fn compute_pagerank(graph: &InMemoryGraph, config: &CentralityConfig) -> Vec<f64> {
    if graph.node_count() == 0 {
        return vec![];
    }
    petgraph::algo::page_rank(&graph.graph, config.damping, config.max_iterations as usize)
}

// ── Betweenness Centrality (Brandes algorithm) ─────────────────────

fn compute_betweenness(graph: &InMemoryGraph, config: &CentralityConfig) -> Vec<f64> {
    let n = graph.node_count();
    if n == 0 {
        return vec![];
    }

    let use_approx = n > config.approx_threshold;
    let k = if use_approx {
        config
            .approx_k
            .unwrap_or_else(|| (n as f64).sqrt() as usize)
    } else {
        n
    };

    if use_approx {
        info!(n, k, "Using approximate betweenness (k-source sampling)");
    }

    brandes_betweenness(&graph.graph, k)
}

/// Brandes' algorithm for betweenness centrality.
/// If `k < n`, only `k` randomly-chosen source nodes are used (approximation).
fn brandes_betweenness(graph: &DiGraph<NodeId, f64>, k: usize) -> Vec<f64> {
    let n = graph.node_count();
    if n == 0 {
        return vec![];
    }

    let mut cb = vec![0.0_f64; n];

    // Choose source nodes (all for exact, subset for approx)
    let sources: Vec<NodeIndex> = if k >= n {
        graph.node_indices().collect()
    } else {
        // Deterministic sampling: evenly spaced nodes
        let step = n / k;
        graph
            .node_indices()
            .step_by(step.max(1))
            .take(k)
            .collect()
    };

    for &s in &sources {
        let s_idx = s.index();

        // BFS from s
        let mut stack: Vec<NodeIndex> = Vec::new();
        let mut predecessors: Vec<Vec<NodeIndex>> = vec![vec![]; n];
        let mut sigma = vec![0.0_f64; n]; // number of shortest paths
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

                // First visit?
                if dist[w_idx] < 0 {
                    dist[w_idx] = dist[v_idx] + 1;
                    queue.push_back(neighbor);
                }

                // Shortest path via v?
                if dist[w_idx] == dist[v_idx] + 1 {
                    sigma[w_idx] += sigma[v_idx];
                    predecessors[w_idx].push(v);
                }
            }
        }

        // Back-propagation of dependencies
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

    // Normalize: if approximate, scale by n/k
    let scale = if k < n {
        n as f64 / k as f64
    } else {
        1.0
    };

    // Normalize to [0, 1] range
    let max_cb = cb.iter().copied().fold(0.0_f64, f64::max);
    if max_cb > 0.0 {
        cb.iter().map(|&v| (v * scale) / (max_cb * scale)).collect()
    } else {
        cb
    }
}

// ── HITS (Hyperlink-Induced Topic Search) ──────────────────────────

fn compute_hits(graph: &InMemoryGraph, config: &CentralityConfig) -> (Vec<f64>, Vec<f64>) {
    let n = graph.node_count();
    if n == 0 {
        return (vec![], vec![]);
    }

    hits_power_iteration(&graph.graph, config.max_iterations as usize)
}

/// HITS algorithm via power iteration.
/// Returns (`hub_scores`, `authority_scores`), both normalized to [0, 1].
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
        // Authority update: auth(v) = sum of hub(u) for all u→v
        let mut new_auth = vec![0.0_f64; n];
        for edge in graph.edge_indices() {
            if let Some((src, tgt)) = graph.edge_endpoints(edge) {
                new_auth[tgt.index()] += hubs[src.index()];
            }
        }

        // Hub update: hub(u) = sum of auth(v) for all u→v
        let mut new_hub = vec![0.0_f64; n];
        for edge in graph.edge_indices() {
            if let Some((src, tgt)) = graph.edge_endpoints(edge) {
                new_hub[src.index()] += new_auth[tgt.index()];
            }
        }

        // Normalize
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

        // Check convergence
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

    // Normalize to [0, 1] by dividing by max
    let max_hub = hubs.iter().copied().fold(0.0_f64, f64::max);
    let max_auth = authorities.iter().copied().fold(0.0_f64, f64::max);

    if max_hub > 0.0 {
        for h in &mut hubs {
            *h /= max_hub;
        }
    }
    if max_auth > 0.0 {
        for a in &mut authorities {
            *a /= max_auth;
        }
    }

    (hubs, authorities)
}

// ── Storage helpers ────────────────────────────────────────────────

async fn store_centrality_results(
    store: &dyn HomerStore,
    graph: &InMemoryGraph,
    scores: &[f64],
    kind: AnalysisKind,
    score_field: &str,
) -> crate::error::Result<u64> {
    if scores.is_empty() {
        return Ok(0);
    }

    // Compute ranks (1-indexed, highest score = rank 1)
    let mut indexed: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut ranks = vec![0u32; scores.len()];
    for (rank, (idx, _)) in indexed.iter().enumerate() {
        ranks[*idx] = (rank + 1) as u32;
    }

    let now = Utc::now();
    let mut count = 0u64;

    for (node_idx, &node_id) in &graph.index_to_node {
        let idx = node_idx.index();
        if idx >= scores.len() {
            continue;
        }

        let node_score = scores[idx];
        let rank = ranks[idx];

        // Count in/out degree
        let in_degree = graph
            .graph
            .neighbors_directed(*node_idx, petgraph::Direction::Incoming)
            .count() as u32;
        let out_degree = graph
            .graph
            .neighbors_directed(*node_idx, petgraph::Direction::Outgoing)
            .count() as u32;

        let data = serde_json::json!({
            score_field: node_score,
            "rank": rank,
            "in_degree": in_degree,
            "out_degree": out_degree,
        });

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind: kind.clone(),
            data,
            input_hash: 0,
            computed_at: now,
        };
        store.store_analysis(&result).await?;
        count += 1;
    }

    Ok(count)
}

async fn store_hits_results(
    store: &dyn HomerStore,
    graph: &InMemoryGraph,
    hub_scores: &[f64],
    authority_scores: &[f64],
) -> crate::error::Result<u64> {
    if hub_scores.is_empty() {
        return Ok(0);
    }

    let now = Utc::now();
    let mut count = 0u64;

    for (node_idx, &node_id) in &graph.index_to_node {
        let idx = node_idx.index();
        if idx >= hub_scores.len() {
            continue;
        }

        let hub = hub_scores[idx];
        let auth = authority_scores[idx];

        let classification = if hub > 0.5 && auth > 0.5 {
            "Both"
        } else if hub > 0.5 {
            "Hub"
        } else if auth > 0.5 {
            "Authority"
        } else {
            "Neither"
        };

        let data = serde_json::json!({
            "hub_score": hub,
            "authority_score": auth,
            "classification": classification,
        });

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind: AnalysisKind::HITSScore,
            data,
            input_hash: 0,
            computed_at: now,
        };
        store.store_analysis(&result).await?;
        count += 1;
    }

    Ok(count)
}

// ── Composite Salience ─────────────────────────────────────────────

/// Weights for composite salience score.
const W_PAGERANK: f64 = 0.30;
const W_BETWEENNESS: f64 = 0.15;
const W_AUTHORITY: f64 = 0.15;
const W_CHANGE_FREQ: f64 = 0.15;
const W_BUS_FACTOR_RISK: f64 = 0.10;
const W_CODE_SIZE: f64 = 0.05;
const W_TEST_PRESENCE: f64 = 0.10;

async fn compute_and_store_salience(
    store: &dyn HomerStore,
    call_graph: &InMemoryGraph,
    import_graph: &InMemoryGraph,
    pagerank_scores: &[f64],
    betweenness_scores: &[f64],
    authority_scores: &[f64],
) -> crate::error::Result<u64> {
    // Collect all unique node IDs across both graphs
    let mut all_nodes: HashMap<NodeId, SalienceInputs> = HashMap::new();

    // PageRank scores (from call graph)
    for (node_idx, &node_id) in &call_graph.index_to_node {
        let idx = node_idx.index();
        if idx < pagerank_scores.len() {
            all_nodes.entry(node_id).or_default().pagerank = pagerank_scores[idx];
        }
        if idx < authority_scores.len() {
            all_nodes.entry(node_id).or_default().authority = authority_scores[idx];
        }
    }

    // Betweenness scores (from import graph)
    for (node_idx, &node_id) in &import_graph.index_to_node {
        let idx = node_idx.index();
        if idx < betweenness_scores.len() {
            all_nodes.entry(node_id).or_default().betweenness = betweenness_scores[idx];
        }
    }

    // Enrich with behavioral data from store
    let freq_results = store
        .get_analyses_by_kind(AnalysisKind::ChangeFrequency)
        .await?;
    for result in &freq_results {
        if let Some(inputs) = all_nodes.get_mut(&result.node_id) {
            inputs.change_frequency = result
                .data
                .get("percentile")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
        }
    }

    let bus_results = store
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await?;
    for result in &bus_results {
        if let Some(inputs) = all_nodes.get_mut(&result.node_id) {
            let bus_factor = result
                .data
                .get("bus_factor")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as f64;
            // Inverse: lower bus factor = higher risk (0.0 → 1.0 scale)
            inputs.bus_factor_risk = if bus_factor <= 1.0 {
                1.0
            } else {
                1.0 / bus_factor
            };
        }
    }

    // Store salience results
    let now = Utc::now();
    let mut count = 0u64;

    for (&node_id, inputs) in &all_nodes {
        let salience = inputs.pagerank * W_PAGERANK
            + inputs.betweenness * W_BETWEENNESS
            + inputs.authority * W_AUTHORITY
            + inputs.change_frequency * W_CHANGE_FREQ
            + inputs.bus_factor_risk * W_BUS_FACTOR_RISK
            + inputs.code_size * W_CODE_SIZE
            + inputs.test_presence * W_TEST_PRESENCE;

        let classification = classify_salience(inputs.pagerank, inputs.change_frequency);

        let data = serde_json::json!({
            "score": salience,
            "classification": classification,
            "components": {
                "pagerank": inputs.pagerank,
                "betweenness": inputs.betweenness,
                "authority": inputs.authority,
                "change_frequency": inputs.change_frequency,
                "bus_factor_risk": inputs.bus_factor_risk,
                "code_size": inputs.code_size,
                "test_presence": inputs.test_presence,
            }
        });

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind: AnalysisKind::CompositeSalience,
            data,
            input_hash: 0,
            computed_at: now,
        };
        store.store_analysis(&result).await?;
        count += 1;
    }

    Ok(count)
}

#[derive(Debug, Default)]
struct SalienceInputs {
    pagerank: f64,
    betweenness: f64,
    authority: f64,
    change_frequency: f64,
    bus_factor_risk: f64,
    code_size: f64,
    test_presence: f64,
}

fn classify_salience(centrality: f64, churn: f64) -> &'static str {
    let high_centrality = centrality > 0.5;
    let high_churn = churn > 0.5;

    match (high_centrality, high_churn) {
        (true, true) => "ActiveHotspot",
        (true, false) => "FoundationalStable",
        (false, true) => "PeripheralActive",
        (false, false) => "QuietLeaf",
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Hyperedge, HyperedgeId, HyperedgeMember, Node, NodeKind};

    async fn setup_call_graph(store: &SqliteStore) {
        // Create a simple call graph:
        //   main → greet → format_name
        //   main → add
        //   greet → println!
        let now = Utc::now();

        let nodes = vec![
            ("main", NodeKind::Function),
            ("greet", NodeKind::Function),
            ("format_name", NodeKind::Function),
            ("add", NodeKind::Function),
            ("println!", NodeKind::Function),
        ];

        for (name, kind) in nodes {
            let node = Node {
                id: NodeId(0),
                kind,
                name: name.to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            };
            store.upsert_node(&node).await.unwrap();
        }

        let calls = vec![
            ("main", "greet"),
            ("main", "add"),
            ("greet", "format_name"),
            ("greet", "println!"),
        ];

        for (caller, callee) in &calls {
            let src = store
                .get_node_by_name(NodeKind::Function, caller)
                .await
                .unwrap()
                .unwrap();
            let tgt = store
                .get_node_by_name(NodeKind::Function, callee)
                .await
                .unwrap()
                .unwrap();

            let edge = Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Calls,
                members: vec![
                    HyperedgeMember {
                        node_id: src.id,
                        role: "caller".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: tgt.id,
                        role: "callee".to_string(),
                        position: 1,
                    },
                ],
                confidence: 0.7,
                last_updated: now,
                metadata: HashMap::new(),
            };
            store.upsert_hyperedge(&edge).await.unwrap();
        }
    }

    async fn setup_import_graph(store: &SqliteStore) {
        let now = Utc::now();

        // Import graph: auth → utils, payment → utils, payment → auth, api → auth
        let modules = vec![
            ("auth", NodeKind::Module),
            ("utils", NodeKind::Module),
            ("payment", NodeKind::Module),
            ("api", NodeKind::Module),
        ];

        for (name, kind) in modules {
            let node = Node {
                id: NodeId(0),
                kind,
                name: name.to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            };
            store.upsert_node(&node).await.unwrap();
        }

        let imports = vec![
            ("auth", "utils"),
            ("payment", "utils"),
            ("payment", "auth"),
            ("api", "auth"),
        ];

        for (importer, imported) in &imports {
            let src_node = store
                .get_node_by_name(NodeKind::Module, importer)
                .await
                .unwrap()
                .unwrap();
            let tgt_node = store
                .get_node_by_name(NodeKind::Module, imported)
                .await
                .unwrap()
                .unwrap();

            let edge = Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Imports,
                members: vec![
                    HyperedgeMember {
                        node_id: src_node.id,
                        role: "source".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: tgt_node.id,
                        role: "target".to_string(),
                        position: 1,
                    },
                ],
                confidence: 0.8,
                last_updated: now,
                metadata: HashMap::new(),
            };
            store.upsert_hyperedge(&edge).await.unwrap();
        }
    }

    #[tokio::test]
    async fn pagerank_on_call_graph() {
        let store = SqliteStore::in_memory().unwrap();
        setup_call_graph(&store).await;

        let graph = InMemoryGraph::from_store(&store, HyperedgeKind::Calls)
            .await
            .unwrap();
        assert_eq!(graph.node_count(), 5);
        assert_eq!(graph.edge_count(), 4);

        let config = CentralityConfig::default();
        let scores = compute_pagerank(&graph, &config);

        assert_eq!(scores.len(), 5);

        // Leaf nodes (format_name, add, println!) should have higher PageRank
        // because they are called but don't call anything, so they absorb rank.
        let main_node = store
            .get_node_by_name(NodeKind::Function, "main")
            .await
            .unwrap()
            .unwrap();
        let greet_node = store
            .get_node_by_name(NodeKind::Function, "greet")
            .await
            .unwrap()
            .unwrap();
        let format_node = store
            .get_node_by_name(NodeKind::Function, "format_name")
            .await
            .unwrap()
            .unwrap();

        let main_idx = graph.node_to_index[&main_node.id].index();
        let greet_idx = graph.node_to_index[&greet_node.id].index();
        let format_idx = graph.node_to_index[&format_node.id].index();

        // format_name (called by greet, calls nothing) should have reasonable rank
        assert!(scores[format_idx] > 0.0, "format_name should have score");
        // greet (called by main, calls format_name + println!) acts as hub
        assert!(scores[greet_idx] > 0.0, "greet should have score");
        // main (calls things, called by nothing) should have lower PageRank
        assert!(
            scores[main_idx] < scores[greet_idx] || scores[main_idx] < scores[format_idx],
            "main (root caller) should have lower PageRank than called functions"
        );
    }

    #[tokio::test]
    async fn betweenness_on_import_graph() {
        let store = SqliteStore::in_memory().unwrap();
        setup_import_graph(&store).await;

        let graph = InMemoryGraph::from_store(&store, HyperedgeKind::Imports)
            .await
            .unwrap();
        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 4);

        let config = CentralityConfig::default();
        let scores = compute_betweenness(&graph, &config);

        assert_eq!(scores.len(), 4);

        // auth should have highest betweenness (bridge between api/payment and utils)
        let auth_node = store
            .get_node_by_name(NodeKind::Module, "auth")
            .await
            .unwrap()
            .unwrap();
        let auth_idx = graph.node_to_index[&auth_node.id].index();

        assert!(
            scores[auth_idx] > 0.0,
            "auth should have positive betweenness (it's a bridge)"
        );
    }

    #[tokio::test]
    async fn hits_on_call_graph() {
        let store = SqliteStore::in_memory().unwrap();
        setup_call_graph(&store).await;

        let graph = InMemoryGraph::from_store(&store, HyperedgeKind::Calls)
            .await
            .unwrap();

        let config = CentralityConfig::default();
        let (hub_scores, auth_scores) = compute_hits(&graph, &config);

        assert_eq!(hub_scores.len(), 5);
        assert_eq!(auth_scores.len(), 5);

        // main and greet should be hubs (they call others)
        let main_node = store
            .get_node_by_name(NodeKind::Function, "main")
            .await
            .unwrap()
            .unwrap();
        let greet_node = store
            .get_node_by_name(NodeKind::Function, "greet")
            .await
            .unwrap()
            .unwrap();
        let format_node = store
            .get_node_by_name(NodeKind::Function, "format_name")
            .await
            .unwrap()
            .unwrap();

        let main_idx = graph.node_to_index[&main_node.id].index();
        let greet_idx = graph.node_to_index[&greet_node.id].index();
        let format_idx = graph.node_to_index[&format_node.id].index();

        // format_name (leaf, called but doesn't call) should be an authority
        assert!(
            auth_scores[format_idx] > hub_scores[format_idx],
            "format_name should be more authority than hub"
        );

        // main (calls others, not called) should be a hub
        assert!(
            hub_scores[main_idx] > auth_scores[main_idx],
            "main should be more hub than authority"
        );

        // greet (calls and is called) should have both scores
        assert!(hub_scores[greet_idx] > 0.0, "greet should be a hub");
        assert!(auth_scores[greet_idx] > 0.0, "greet should be an authority");
    }

    #[tokio::test]
    async fn full_centrality_analysis() {
        let store = SqliteStore::in_memory().unwrap();
        setup_call_graph(&store).await;
        setup_import_graph(&store).await;

        let analyzer = CentralityAnalyzer::default();
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        // Should have stored results for PageRank, Betweenness, HITS, CompositeSalience
        assert!(
            stats.results_stored > 0,
            "Should store centrality results, got 0"
        );

        // Verify PageRank results in store
        let pr_results = store
            .get_analyses_by_kind(AnalysisKind::PageRank)
            .await
            .unwrap();
        assert_eq!(pr_results.len(), 5, "Should have PageRank for each function");

        // Verify HITS results
        let hits_results = store
            .get_analyses_by_kind(AnalysisKind::HITSScore)
            .await
            .unwrap();
        assert_eq!(hits_results.len(), 5, "Should have HITS for each function");

        // Verify betweenness results
        let bc_results = store
            .get_analyses_by_kind(AnalysisKind::BetweennessCentrality)
            .await
            .unwrap();
        assert_eq!(bc_results.len(), 4, "Should have betweenness for each module");

        // Verify composite salience (should cover nodes from both graphs)
        let salience_results = store
            .get_analyses_by_kind(AnalysisKind::CompositeSalience)
            .await
            .unwrap();
        assert!(
            salience_results.len() >= 5,
            "Should have salience for at least all call graph nodes, got {}",
            salience_results.len()
        );

        // Check salience has valid structure
        for r in &salience_results {
            let has_score = r.data.get("score").and_then(serde_json::Value::as_f64);
            assert!(has_score.is_some(), "Salience should have score field");
            let classification = r
                .data
                .get("classification")
                .and_then(serde_json::Value::as_str);
            assert!(
                classification.is_some(),
                "Salience should have classification"
            );
        }
    }

    #[tokio::test]
    async fn empty_graph_no_panic() {
        let store = SqliteStore::in_memory().unwrap();

        let analyzer = CentralityAnalyzer::default();
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        assert_eq!(stats.results_stored, 0, "Empty graph should produce 0 results");
    }

    #[test]
    fn brandes_known_graph() {
        // Linear graph: A → B → C
        let mut graph = DiGraph::<NodeId, f64>::new();
        let a = graph.add_node(NodeId(1));
        let b = graph.add_node(NodeId(2));
        let c = graph.add_node(NodeId(3));
        graph.add_edge(a, b, 1.0);
        graph.add_edge(b, c, 1.0);

        let scores = brandes_betweenness(&graph, 3);

        // B is on the shortest path A→C, so B should have the highest betweenness
        assert!(
            scores[b.index()] > scores[a.index()],
            "B (bridge) should have higher betweenness than A"
        );
        assert!(
            scores[b.index()] > scores[c.index()],
            "B (bridge) should have higher betweenness than C"
        );
    }

    #[test]
    fn hits_known_graph() {
        // Star graph: A → B, A → C, A → D (A is hub, B/C/D are authorities)
        let mut graph = DiGraph::<NodeId, f64>::new();
        let a = graph.add_node(NodeId(1));
        let b = graph.add_node(NodeId(2));
        let c = graph.add_node(NodeId(3));
        let d = graph.add_node(NodeId(4));
        graph.add_edge(a, b, 1.0);
        graph.add_edge(a, c, 1.0);
        graph.add_edge(a, d, 1.0);

        let (hubs, auths) = hits_power_iteration(&graph, 100);

        // A should be the only hub
        assert!(
            (hubs[a.index()] - 1.0).abs() < f64::EPSILON,
            "A should be the top hub"
        );
        assert!(hubs[b.index()] < 0.01, "B should not be a hub");

        // B, C, D should all be equal authorities
        assert!(auths[b.index()] > 0.5, "B should be an authority");
        let diff = (auths[b.index()] - auths[c.index()]).abs();
        assert!(diff < 0.01, "B and C should have equal authority scores");
    }

    #[test]
    fn salience_classification() {
        assert_eq!(classify_salience(0.8, 0.8), "ActiveHotspot");
        assert_eq!(classify_salience(0.8, 0.2), "FoundationalStable");
        assert_eq!(classify_salience(0.2, 0.8), "PeripheralActive");
        assert_eq!(classify_salience(0.2, 0.2), "QuietLeaf");
    }
}
