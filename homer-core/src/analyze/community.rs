// Community detection (Louvain) and stability classification.
//
// Graph algorithms intentionally cast int↔float.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use petgraph::graph::NodeIndex;
use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind};

use super::AnalyzeStats;
use super::centrality::InMemoryGraph;
use super::traits::Analyzer;

// ── Community Detection (Louvain) ──────────────────────────────────

/// Louvain community detection on a graph.
/// Returns a map from `NodeIndex` → community ID.
pub fn louvain_communities(graph: &InMemoryGraph) -> HashMap<NodeIndex, u32> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    // Build undirected adjacency with weights (treat directed as undirected)
    let mut adj: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
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
        // No edges — each node is its own community
        return graph
            .graph
            .node_indices()
            .enumerate()
            .map(|(i, idx)| (idx, i as u32))
            .collect();
    }

    // Initialize: each node in its own community
    let mut community: Vec<u32> = (0..n).map(|i| i as u32).collect();

    // Compute node weights (sum of incident edge weights)
    let mut node_weight: Vec<f64> = vec![0.0; n];
    for (&node, neighbors) in &adj {
        for &(_, w) in neighbors {
            node_weight[node] += w;
        }
    }

    // Louvain phase 1: local moves
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
                    let neighbor_comm = community[neighbor];
                    *comm_weights.entry(neighbor_comm).or_default() += weight;
                }
            }

            // Compute community totals
            let mut comm_totals: HashMap<u32, f64> = HashMap::new();
            for (i, &c) in community.iter().enumerate() {
                *comm_totals.entry(c).or_default() += node_weight[i];
            }

            let ki = node_weight[node];
            let m2 = 2.0 * total_weight;

            // Modularity gain from removing node from current community
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
            }
        }
    }

    // Renumber communities to be contiguous from 0
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

    // Map back to NodeIndex → community
    graph
        .graph
        .node_indices()
        .map(|idx| (idx, community[idx.index()]))
        .collect()
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

// ── Analyzer ───────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct CommunityAnalyzer;

#[async_trait::async_trait]
impl Analyzer for CommunityAnalyzer {
    fn name(&self) -> &'static str {
        "community"
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Load import graph for community detection
        let import_graph =
            InMemoryGraph::from_store(store, HyperedgeKind::Imports).await?;

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

        // ── Community Detection ─────────────────────────────────────
        let communities = louvain_communities(&import_graph);

        // Build community → names map for directory alignment
        let mut community_names: HashMap<u32, Vec<String>> = HashMap::new();
        for (node_idx, &comm) in &communities {
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

        for (&node_idx, &comm) in &communities {
            if let Some(&node_id) = import_graph.index_to_node.get(&node_idx) {
                let node_name = store
                    .get_node(node_id)
                    .await?
                    .map_or_else(String::new, |n| n.name.clone());

                let aligned = check_directory_alignment(&node_name, comm, &community_names);

                let data = serde_json::json!({
                    "community_id": comm,
                    "directory_aligned": aligned,
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
            "Community detection complete"
        );

        // ── Stability Classification ────────────────────────────────
        // Combine centrality + churn for each node that has both signals
        let salience_results = store
            .get_analyses_by_kind(AnalysisKind::CompositeSalience)
            .await?;

        let mut stability_count = 0u64;
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

            let data = serde_json::json!({
                "classification": classification,
                "centrality": centrality,
                "churn": churn,
            });

            let result = AnalysisResult {
                id: AnalysisResultId(0),
                node_id: sr.node_id,
                kind: AnalysisKind::StabilityClassification,
                data,
                input_hash: 0,
                computed_at: now,
            };
            store.store_analysis(&result).await?;
            stability_count += 1;
        }
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
                ["StableCore", "ActiveCritical", "ReliableBackground", "Volatile"]
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
}
