// Community detection (Louvain) and stability classification.
//
// Graph algorithms intentionally cast int↔float.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::time::Instant;

use chrono::Utc;
use petgraph::graph::NodeIndex;
use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind};

use super::AnalyzeStats;
use super::centrality::InMemoryGraph;
use super::traits::Analyzer;

/// Adjacency list: node index → list of (neighbor, weight).
type AdjList = HashMap<usize, Vec<(usize, f64)>>;

// ── Community Detection (Louvain) ──────────────────────────────────

/// Result of Louvain community detection.
#[derive(Debug)]
pub struct LouvainResult {
    /// Map from `NodeIndex` → community ID.
    pub communities: HashMap<NodeIndex, u32>,
    /// Overall modularity score (Q).
    pub modularity: f64,
    /// Number of multi-level passes performed.
    pub levels: u32,
}

/// Louvain community detection with multi-level graph contraction.
///
/// Phase 1: Local moves — greedily reassign nodes to neighboring communities.
/// Phase 2: Contract — merge communities into super-nodes, sum edge weights.
/// Repeat until modularity no longer improves.
pub fn louvain_communities(graph: &InMemoryGraph) -> HashMap<NodeIndex, u32> {
    louvain_full(graph).communities
}

/// Full Louvain with metadata (modularity score, levels).
pub fn louvain_full(graph: &InMemoryGraph) -> LouvainResult {
    let n = graph.node_count();
    if n == 0 {
        return LouvainResult {
            communities: HashMap::new(),
            modularity: 0.0,
            levels: 0,
        };
    }

    // Build undirected adjacency with weights (treat directed as undirected)
    let mut adj: AdjList = HashMap::new();
    let mut total_weight = 0.0_f64;

    for edge_idx in graph.graph.edge_indices() {
        if let Some((src, tgt)) = graph.graph.edge_endpoints(edge_idx) {
            let w = graph.graph[edge_idx];
            let weight = if w > 0.0 { w } else { 1.0 };

            adj.entry(src.index())
                .or_default()
                .push((tgt.index(), weight));
            adj.entry(tgt.index())
                .or_default()
                .push((src.index(), weight));
            total_weight += weight;
        }
    }

    if total_weight == 0.0 {
        let communities = graph
            .graph
            .node_indices()
            .enumerate()
            .map(|(i, idx)| (idx, i as u32))
            .collect();
        return LouvainResult {
            communities,
            modularity: 0.0,
            levels: 0,
        };
    }

    // Multi-level Louvain: track which original nodes belong to which super-node
    // `membership[i]` = current community of original node i
    let mut membership: Vec<u32> = (0..n).map(|i| i as u32).collect();
    let mut current_adj = adj.clone();
    let mut current_n = n;
    let mut current_total_weight = total_weight;
    let mut levels = 0u32;
    let max_levels = 10;

    loop {
        if levels >= max_levels {
            break;
        }

        // Phase 1: local moves on current (possibly contracted) graph
        let (community, improved) = louvain_phase1(&current_adj, current_n, current_total_weight);

        if !improved && levels > 0 {
            break;
        }

        levels += 1;

        // Map original node memberships through this level's assignments
        // Each original node's community = community[super_node_it_belongs_to]
        for m in &mut membership {
            *m = community[*m as usize];
        }

        // Count distinct communities
        let num_communities = {
            let unique: HashSet<u32> = community.iter().copied().collect();
            unique.len()
        };

        // If no contraction possible (every node in own community, or single community), stop
        if num_communities >= current_n || num_communities <= 1 {
            break;
        }

        // Phase 2: contract graph — merge nodes in same community into super-nodes
        let (contracted_adj, contracted_n, contracted_weight) =
            contract_graph(&current_adj, &community, current_n);

        current_adj = contracted_adj;
        current_n = contracted_n;
        current_total_weight = contracted_weight;
    }

    // Renumber communities to be contiguous from 0
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut next_id = 0u32;
    for m in &mut membership {
        let new_id = *remap.entry(*m).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        *m = new_id;
    }

    // Compute final modularity
    let modularity = compute_modularity(&membership, &adj, n, total_weight);

    // Map back to NodeIndex → community
    let communities = graph
        .graph
        .node_indices()
        .map(|idx| (idx, membership[idx.index()]))
        .collect();

    LouvainResult {
        communities,
        modularity,
        levels,
    }
}

/// Louvain Phase 1: greedy local moves. Returns `(community assignment, did_improve)`.
fn louvain_phase1(adj: &AdjList, n: usize, total_weight: f64) -> (Vec<u32>, bool) {
    let mut community: Vec<u32> = (0..n).map(|i| i as u32).collect();

    // Compute node weights (sum of incident edge weights)
    let mut node_weight: Vec<f64> = vec![0.0; n];
    for (&node, neighbors) in adj {
        if node < n {
            for &(_, w) in neighbors {
                node_weight[node] += w;
            }
        }
    }

    let mut any_improved = false;
    let mut improved = true;
    let mut iterations = 0;
    let max_iterations = 20;

    while improved && iterations < max_iterations {
        improved = false;
        iterations += 1;

        for node in 0..n {
            let current_comm = community[node];

            // Compute weights to each neighboring community
            let mut comm_weights: HashMap<u32, f64> = HashMap::new();
            if let Some(neighbors) = adj.get(&node) {
                for &(neighbor, weight) in neighbors {
                    if neighbor < n {
                        let neighbor_comm = community[neighbor];
                        *comm_weights.entry(neighbor_comm).or_default() += weight;
                    }
                }
            }

            // Compute community totals
            let mut comm_totals: HashMap<u32, f64> = HashMap::new();
            for (i, &c) in community.iter().enumerate() {
                *comm_totals.entry(c).or_default() += node_weight[i];
            }

            let ki = node_weight[node];
            let m2 = 2.0 * total_weight;

            let ki_in_current = comm_weights.get(&current_comm).copied().unwrap_or(0.0);
            let sigma_current = comm_totals.get(&current_comm).copied().unwrap_or(0.0);

            let mut best_gain = 0.0_f64;
            let mut best_comm = current_comm;

            for (&target_comm, &ki_in_target) in &comm_weights {
                if target_comm == current_comm {
                    continue;
                }

                let sigma_target = comm_totals.get(&target_comm).copied().unwrap_or(0.0);

                // Modularity gain = [ki_in_target/m - sigma_target*ki/m^2]
                //                  - [ki_in_current/m - (sigma_current - ki)*ki/m^2]
                let gain = (ki_in_target - ki_in_current) / m2
                    + ki * ((sigma_current - ki) - sigma_target) / (m2 * m2) * 2.0;

                if gain > best_gain {
                    best_gain = gain;
                    best_comm = target_comm;
                }
            }

            if best_comm != current_comm {
                community[node] = best_comm;
                improved = true;
                any_improved = true;
            }
        }
    }

    // Renumber to contiguous IDs for contraction
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut next_id = 0u32;
    for c in &mut community {
        let new_id = *remap.entry(*c).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        *c = new_id;
    }

    (community, any_improved)
}

/// Louvain Phase 2: contract graph by merging nodes in the same community.
///
/// Returns (new adjacency, number of super-nodes, total weight).
/// Self-loops (intra-community edges) are preserved — they represent internal
/// cohesion and are essential for correct modularity computation in later passes.
fn contract_graph(adj: &AdjList, community: &[u32], _n: usize) -> (AdjList, usize, f64) {
    let num_communities = community.iter().copied().max().map_or(0, |m| m + 1) as usize;

    // Build super-node adjacency: aggregate edge weights between communities
    let mut super_adj: HashMap<(usize, usize), f64> = HashMap::new();

    for (&node, neighbors) in adj {
        if node >= community.len() {
            continue;
        }
        let src_comm = community[node] as usize;
        for &(neighbor, weight) in neighbors {
            if neighbor >= community.len() {
                continue;
            }
            let dst_comm = community[neighbor] as usize;
            if src_comm <= dst_comm {
                *super_adj.entry((src_comm, dst_comm)).or_default() += weight;
            }
        }
    }

    // Convert to adjacency list format, including self-loops
    let mut new_adj: AdjList = HashMap::new();
    let mut total_weight = 0.0_f64;

    for (&(a, b), &weight) in &super_adj {
        if a == b {
            // Self-loops: add to adjacency (both directions = itself) for node weight calc
            new_adj.entry(a).or_default().push((a, weight));
            total_weight += weight / 2.0;
        } else {
            new_adj.entry(a).or_default().push((b, weight));
            new_adj.entry(b).or_default().push((a, weight));
            total_weight += weight / 2.0;
        }
    }

    (new_adj, num_communities, total_weight)
}

/// Compute modularity Q for a given community assignment.
fn compute_modularity(community: &[u32], adj: &AdjList, n: usize, total_weight: f64) -> f64 {
    if total_weight == 0.0 {
        return 0.0;
    }

    let m2 = 2.0 * total_weight;

    // Node degrees
    let mut degree: Vec<f64> = vec![0.0; n];
    for (&node, neighbors) in adj {
        if node < n {
            for &(_, w) in neighbors {
                degree[node] += w;
            }
        }
    }

    let mut q = 0.0_f64;
    for (&node, neighbors) in adj {
        if node >= n {
            continue;
        }
        for &(neighbor, weight) in neighbors {
            if neighbor >= n {
                continue;
            }
            if community[node] == community[neighbor] {
                q += weight - (degree[node] * degree[neighbor]) / m2;
            }
        }
    }

    q / m2
}

/// Compute per-node modularity contribution.
pub fn modularity_contributions<S: BuildHasher>(
    communities: &HashMap<NodeIndex, u32, S>,
    graph: &InMemoryGraph,
) -> HashMap<NodeIndex, f64> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    // Build adjacency
    let mut adj: AdjList = HashMap::new();
    let mut total_weight = 0.0_f64;

    for edge_idx in graph.graph.edge_indices() {
        if let Some((src, tgt)) = graph.graph.edge_endpoints(edge_idx) {
            let w = graph.graph[edge_idx];
            let weight = if w > 0.0 { w } else { 1.0 };
            adj.entry(src.index())
                .or_default()
                .push((tgt.index(), weight));
            adj.entry(tgt.index())
                .or_default()
                .push((src.index(), weight));
            total_weight += weight;
        }
    }

    if total_weight == 0.0 {
        return graph.graph.node_indices().map(|idx| (idx, 0.0)).collect();
    }

    let m2 = 2.0 * total_weight;

    // Degree per node
    let mut degree: Vec<f64> = vec![0.0; n];
    for (&node, neighbors) in &adj {
        if node < n {
            for &(_, w) in neighbors {
                degree[node] += w;
            }
        }
    }

    // Community assignment as vector
    let mut comm_vec: Vec<u32> = vec![0; n];
    for (&idx, &c) in communities {
        if idx.index() < n {
            comm_vec[idx.index()] = c;
        }
    }

    // Per-node: sum of (A_ij - ki*kj/2m) for same-community neighbors
    let mut contributions: HashMap<NodeIndex, f64> = HashMap::new();
    for idx in graph.graph.node_indices() {
        let node = idx.index();
        let mut contrib = 0.0_f64;

        if let Some(neighbors) = adj.get(&node) {
            for &(neighbor, weight) in neighbors {
                if neighbor < n && comm_vec[node] == comm_vec[neighbor] {
                    contrib += weight - (degree[node] * degree[neighbor]) / m2;
                }
            }
        }

        contributions.insert(idx, contrib / m2);
    }

    contributions
}

/// Check if a node's directory path aligns with its community peers.
fn check_directory_alignment(
    node_name: &str,
    community_id: u32,
    all_names: &HashMap<u32, Vec<String>>,
) -> bool {
    let node_dir = extract_directory(node_name);

    if let Some(peer_names) = all_names.get(&community_id) {
        if peer_names.len() <= 1 {
            return true; // Singleton community — trivially aligned
        }

        let peer_dirs: Vec<&str> = peer_names.iter().map(|n| extract_directory(n)).collect();

        // Node is aligned if its directory matches the majority of peers
        let matching = peer_dirs.iter().filter(|&&d| d == node_dir).count();
        let ratio = matching as f64 / peer_dirs.len() as f64;
        ratio >= 0.5
    } else {
        true
    }
}

fn extract_directory(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

// ── Stability Classification ───────────────────────────────────────

/// Classify stability based on centrality and churn.
fn classify_stability(centrality: f64, churn: f64) -> &'static str {
    let high_centrality = centrality > 0.5;
    let high_churn = churn > 0.5;

    match (high_centrality, high_churn) {
        (true, false) => "StableCore",
        (true, true) => "ActiveCritical",
        (false, false) => "ReliableBackground",
        (false, true) => "Volatile",
    }
}

/// Compute stability classification for each node with salience data.
async fn compute_stability(
    store: &dyn HomerStore,
    now: chrono::DateTime<Utc>,
) -> crate::error::Result<u64> {
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;

    let mut count = 0u64;
    for sr in &salience_results {
        let centrality = sr
            .data
            .get("components")
            .and_then(|c| c.get("pagerank"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        let churn = sr
            .data
            .get("components")
            .and_then(|c| c.get("change_frequency"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        let classification = classify_stability(centrality, churn);

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id: sr.node_id,
            kind: AnalysisKind::StabilityClassification,
            data: serde_json::json!({
                "classification": classification,
                "centrality": centrality,
                "churn": churn,
            }),
            input_hash: 0,
            computed_at: now,
        };
        store.store_analysis(&result).await?;
        count += 1;
    }
    Ok(count)
}

// ── Analyzer ───────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct CommunityAnalyzer;

#[async_trait::async_trait]
impl Analyzer for CommunityAnalyzer {
    fn name(&self) -> &'static str {
        "community"
    }

    #[instrument(skip_all, name = "community_analyze")]
    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Load import graph for community detection
        let import_graph = InMemoryGraph::from_store(store, HyperedgeKind::Imports).await?;

        if import_graph.node_count() == 0 {
            info!("No import graph data, skipping community detection");
            stats.duration = start.elapsed();
            return Ok(stats);
        }

        info!(
            nodes = import_graph.node_count(),
            edges = import_graph.edge_count(),
            "Running Louvain community detection"
        );

        // ── Community Detection (multi-level Louvain) ────────────────
        let louvain = louvain_full(&import_graph);
        let communities = &louvain.communities;

        // Compute per-node modularity contribution
        let contributions = modularity_contributions(communities, &import_graph);

        // Build community → names map for directory alignment
        let mut community_names: HashMap<u32, Vec<String>> = HashMap::new();
        for (node_idx, &comm) in communities {
            if let Some(&node_id) = import_graph.index_to_node.get(node_idx) {
                if let Ok(Some(node)) = store.get_node(node_id).await {
                    community_names
                        .entry(comm)
                        .or_default()
                        .push(node.name.clone());
                }
            }
        }

        let num_communities = community_names.len();

        // Store community assignments
        let now = Utc::now();
        let mut comm_count = 0u64;

        for (&node_idx, &comm) in communities {
            if let Some(&node_id) = import_graph.index_to_node.get(&node_idx) {
                let node_name = store
                    .get_node(node_id)
                    .await?
                    .map_or_else(String::new, |n| n.name.clone());

                let aligned = check_directory_alignment(&node_name, comm, &community_names);
                let mod_contrib = contributions.get(&node_idx).copied().unwrap_or(0.0);

                let data = serde_json::json!({
                    "community_id": comm,
                    "directory_aligned": aligned,
                    "modularity_contribution": (mod_contrib * 1000.0).round() / 1000.0,
                });

                let result = AnalysisResult {
                    id: AnalysisResultId(0),
                    node_id,
                    kind: AnalysisKind::CommunityAssignment,
                    data,
                    input_hash: 0,
                    computed_at: now,
                };
                store.store_analysis(&result).await?;
                comm_count += 1;
            }
        }
        stats.results_stored += comm_count;

        info!(
            communities = num_communities,
            assignments = comm_count,
            modularity = format!("{:.4}", louvain.modularity),
            levels = louvain.levels,
            "Community detection complete"
        );

        // ── Stability Classification ────────────────────────────────
        let stability_count = compute_stability(store, now).await?;
        stats.results_stored += stability_count;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Community + stability analysis complete"
        );

        Ok(stats)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Hyperedge, HyperedgeId, HyperedgeMember, Node, NodeId, NodeKind};

    async fn setup_import_graph(store: &SqliteStore) {
        let now = Utc::now();

        // Create a graph with two natural communities:
        // Community A: auth, session, middleware (tightly connected)
        // Community B: payment, billing, invoice (tightly connected)
        // Bridge: auth → payment (cross-community)
        let modules = [
            "src/auth/mod.rs",
            "src/auth/session.rs",
            "src/auth/middleware.rs",
            "src/payment/mod.rs",
            "src/payment/billing.rs",
            "src/payment/invoice.rs",
        ];

        for name in modules {
            let node = Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: name.to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            };
            store.upsert_node(&node).await.unwrap();
        }

        // Intra-community edges (auth cluster)
        let edges = [
            ("src/auth/mod.rs", "src/auth/session.rs"),
            ("src/auth/mod.rs", "src/auth/middleware.rs"),
            ("src/auth/session.rs", "src/auth/middleware.rs"),
            // Intra-community edges (payment cluster)
            ("src/payment/mod.rs", "src/payment/billing.rs"),
            ("src/payment/mod.rs", "src/payment/invoice.rs"),
            ("src/payment/billing.rs", "src/payment/invoice.rs"),
            // Cross-community bridge
            ("src/auth/mod.rs", "src/payment/mod.rs"),
        ];

        for (from, to) in edges {
            let src = store
                .get_node_by_name(NodeKind::Module, from)
                .await
                .unwrap()
                .unwrap();
            let tgt = store
                .get_node_by_name(NodeKind::Module, to)
                .await
                .unwrap()
                .unwrap();

            let edge = Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Imports,
                members: vec![
                    HyperedgeMember {
                        node_id: src.id,
                        role: "source".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: tgt.id,
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
    async fn louvain_detects_communities() {
        let store = SqliteStore::in_memory().unwrap();
        setup_import_graph(&store).await;

        let graph = InMemoryGraph::from_store(&store, HyperedgeKind::Imports)
            .await
            .unwrap();
        assert_eq!(graph.node_count(), 6);

        let communities = louvain_communities(&graph);
        assert_eq!(communities.len(), 6);

        // The auth nodes should be in the same community
        let auth_mod = store
            .get_node_by_name(NodeKind::Module, "src/auth/mod.rs")
            .await
            .unwrap()
            .unwrap();
        let auth_session = store
            .get_node_by_name(NodeKind::Module, "src/auth/session.rs")
            .await
            .unwrap()
            .unwrap();
        let payment_mod = store
            .get_node_by_name(NodeKind::Module, "src/payment/mod.rs")
            .await
            .unwrap()
            .unwrap();

        let auth_mod_comm = communities[&graph.node_to_index[&auth_mod.id]];
        let auth_sess_comm = communities[&graph.node_to_index[&auth_session.id]];
        let payment_comm = communities[&graph.node_to_index[&payment_mod.id]];

        assert_eq!(
            auth_mod_comm, auth_sess_comm,
            "Auth modules should be in the same community"
        );

        // Count unique communities — should be >= 2
        let unique: std::collections::HashSet<u32> = communities.values().copied().collect();
        assert!(
            unique.len() >= 2,
            "Should detect at least 2 communities, got {}",
            unique.len()
        );

        // If there are exactly 2, auth and payment should differ
        if unique.len() == 2 {
            assert_ne!(
                auth_mod_comm, payment_comm,
                "Auth and payment should be in different communities"
            );
        }
    }

    #[tokio::test]
    async fn directory_alignment_detection() {
        let names: HashMap<u32, Vec<String>> = HashMap::from([
            (
                0,
                vec![
                    "src/auth/mod.rs".to_string(),
                    "src/auth/session.rs".to_string(),
                    "src/auth/middleware.rs".to_string(),
                ],
            ),
            (
                1,
                vec![
                    "src/payment/mod.rs".to_string(),
                    "src/payment/billing.rs".to_string(),
                    "src/auth/validate.rs".to_string(), // misaligned!
                ],
            ),
        ]);

        // Auth module in auth community — aligned
        assert!(check_directory_alignment("src/auth/mod.rs", 0, &names));

        // Auth validate in payment community — misaligned
        assert!(!check_directory_alignment(
            "src/auth/validate.rs",
            1,
            &names
        ));

        // Payment billing in payment community — aligned
        assert!(check_directory_alignment(
            "src/payment/billing.rs",
            1,
            &names
        ));
    }

    #[tokio::test]
    async fn full_community_analysis() {
        let store = SqliteStore::in_memory().unwrap();
        setup_import_graph(&store).await;

        // Run centrality first (community analyzer uses salience for stability)
        let centrality = super::super::centrality::CentralityAnalyzer::default();
        centrality
            .analyze(&store, &HomerConfig::default())
            .await
            .unwrap();

        let analyzer = CommunityAnalyzer;
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        assert!(
            stats.results_stored > 0,
            "Should store community + stability results"
        );

        // Verify community assignments
        let comm_results = store
            .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
            .await
            .unwrap();
        assert_eq!(
            comm_results.len(),
            6,
            "Should have community assignment for each module"
        );

        for r in &comm_results {
            assert!(
                r.data.get("community_id").is_some(),
                "Should have community_id"
            );
            assert!(
                r.data.get("directory_aligned").is_some(),
                "Should have directory_aligned"
            );
            assert!(
                r.data.get("modularity_contribution").is_some(),
                "Should have modularity_contribution"
            );
        }

        // Verify stability classification
        let stability_results = store
            .get_analyses_by_kind(AnalysisKind::StabilityClassification)
            .await
            .unwrap();
        assert!(
            !stability_results.is_empty(),
            "Should have stability classifications"
        );

        for r in &stability_results {
            let cls = r
                .data
                .get("classification")
                .and_then(serde_json::Value::as_str)
                .unwrap();
            assert!(
                [
                    "StableCore",
                    "ActiveCritical",
                    "ReliableBackground",
                    "Volatile"
                ]
                .contains(&cls),
                "Invalid stability: {cls}"
            );
        }
    }

    #[tokio::test]
    async fn empty_graph_no_panic() {
        let store = SqliteStore::in_memory().unwrap();
        let analyzer = CommunityAnalyzer;
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();
        assert_eq!(stats.results_stored, 0);
    }

    #[test]
    fn stability_classification_values() {
        assert_eq!(classify_stability(0.8, 0.2), "StableCore");
        assert_eq!(classify_stability(0.8, 0.8), "ActiveCritical");
        assert_eq!(classify_stability(0.2, 0.2), "ReliableBackground");
        assert_eq!(classify_stability(0.2, 0.8), "Volatile");
    }

    #[test]
    fn extract_directory_works() {
        assert_eq!(extract_directory("src/auth/mod.rs"), "src/auth");
        assert_eq!(extract_directory("lib.rs"), "");
        assert_eq!(extract_directory("src/a/b/c.rs"), "src/a/b");
    }

    #[test]
    fn louvain_multi_level_modularity() {
        use petgraph::graph::DiGraph;

        // Build a graph with 2 clear clusters: {0,1,2} and {3,4,5}
        // Dense intra-cluster edges, sparse inter-cluster
        let mut graph = DiGraph::<NodeId, f64>::new();
        let mut node_to_index = HashMap::new();
        let mut index_to_node = HashMap::new();

        for i in 0..6 {
            let nid = NodeId(i as i64);
            let idx = graph.add_node(nid);
            node_to_index.insert(nid, idx);
            index_to_node.insert(idx, nid);
        }

        // Cluster A: 0-1, 0-2, 1-2 (weight 5.0 each)
        for &(a, b) in &[(0, 1), (0, 2), (1, 2)] {
            let a_idx = node_to_index[&NodeId(a)];
            let b_idx = node_to_index[&NodeId(b)];
            graph.add_edge(a_idx, b_idx, 5.0);
        }

        // Cluster B: 3-4, 3-5, 4-5 (weight 5.0 each)
        for &(a, b) in &[(3, 4), (3, 5), (4, 5)] {
            let a_idx = node_to_index[&NodeId(a)];
            let b_idx = node_to_index[&NodeId(b)];
            graph.add_edge(a_idx, b_idx, 5.0);
        }

        // Bridge: 2-3 (weight 0.5 — weak)
        let idx2 = node_to_index[&NodeId(2)];
        let idx3 = node_to_index[&NodeId(3)];
        graph.add_edge(idx2, idx3, 0.5);

        let im_graph = InMemoryGraph {
            graph,
            node_to_index,
            index_to_node,
        };

        let result = louvain_full(&im_graph);

        // Should detect exactly 2 communities
        let unique: HashSet<u32> = result.communities.values().copied().collect();
        assert_eq!(
            unique.len(),
            2,
            "Should detect 2 communities, got {}",
            unique.len()
        );

        // Nodes 0,1,2 should share a community
        let c0 = result.communities[&im_graph.node_to_index[&NodeId(0)]];
        let c1 = result.communities[&im_graph.node_to_index[&NodeId(1)]];
        let c2 = result.communities[&im_graph.node_to_index[&NodeId(2)]];
        assert_eq!(c0, c1, "Nodes 0 and 1 should be same community");
        assert_eq!(c1, c2, "Nodes 1 and 2 should be same community");

        // Nodes 3,4,5 should share a different community
        let c3 = result.communities[&im_graph.node_to_index[&NodeId(3)]];
        let c4 = result.communities[&im_graph.node_to_index[&NodeId(4)]];
        let c5 = result.communities[&im_graph.node_to_index[&NodeId(5)]];
        assert_eq!(c3, c4, "Nodes 3 and 4 should be same community");
        assert_eq!(c4, c5, "Nodes 4 and 5 should be same community");
        assert_ne!(c0, c3, "Clusters should be different communities");

        // Modularity should be positive (good partition)
        assert!(
            result.modularity > 0.0,
            "Modularity should be positive, got {}",
            result.modularity
        );

        // At least 1 level should have been performed
        assert!(result.levels >= 1, "Should perform at least 1 level");
    }

    #[test]
    fn modularity_contributions_nonzero() {
        use petgraph::graph::DiGraph;

        // Triangle: all in same community → contributions should be positive
        let mut graph = DiGraph::<NodeId, f64>::new();
        let mut node_to_index = HashMap::new();
        let mut index_to_node = HashMap::new();

        for i in 0..3 {
            let nid = NodeId(i as i64);
            let idx = graph.add_node(nid);
            node_to_index.insert(nid, idx);
            index_to_node.insert(idx, nid);
        }

        let idx0 = node_to_index[&NodeId(0)];
        let idx1 = node_to_index[&NodeId(1)];
        let idx2 = node_to_index[&NodeId(2)];
        graph.add_edge(idx0, idx1, 1.0);
        graph.add_edge(idx1, idx2, 1.0);
        graph.add_edge(idx0, idx2, 1.0);

        let im_graph = InMemoryGraph {
            graph,
            node_to_index,
            index_to_node,
        };

        let communities = louvain_communities(&im_graph);
        let contribs = modularity_contributions(&communities, &im_graph);

        assert_eq!(contribs.len(), 3);
        // All in same community with dense edges → positive contributions
        for (_, &c) in &contribs {
            assert!(c >= 0.0, "Contribution should be non-negative, got {c}");
        }
    }
}
