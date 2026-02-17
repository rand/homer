// Temporal analysis: centrality trends, architectural drift, and enhanced stability.
//
// Statistical computations intentionally cast int→float.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind, NodeId,
};

use super::AnalyzeStats;
use super::traits::Analyzer;

// ── Temporal Analyzer ──────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct TemporalAnalyzer;

#[async_trait::async_trait]
impl Analyzer for TemporalAnalyzer {
    fn name(&self) -> &'static str {
        "temporal"
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();
        let now = Utc::now();

        // Compute centrality trends (compare current vs historical)
        let trend_count = compute_centrality_trends(store, now).await?;
        stats.results_stored += trend_count;

        // Compute architectural drift (cross-community coupling)
        let drift_count = compute_architectural_drift(store, now).await?;
        stats.results_stored += drift_count;

        // Enhance stability classification with Declining class
        let stability_count = enhance_stability(store, now).await?;
        stats.results_stored += stability_count;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Temporal analysis complete"
        );
        Ok(stats)
    }
}

// ── Centrality Trends ─────────────────────────────────────────────

/// Compare current centrality scores with previous snapshot to detect trends.
///
/// Uses the `input_hash` field to store a generation counter. Each run
/// increments the generation; trend is computed from the last two generations.
async fn compute_centrality_trends(
    store: &dyn HomerStore,
    now: chrono::DateTime<Utc>,
) -> crate::error::Result<u64> {
    // Load current composite salience scores (which embed PageRank)
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;

    if salience_results.is_empty() {
        return Ok(0);
    }

    // Load existing centrality trend results (if any) for history
    let existing_trends = store
        .get_analyses_by_kind(AnalysisKind::CentralityTrend)
        .await?;

    let history: HashMap<NodeId, Vec<f64>> = existing_trends
        .into_iter()
        .map(|r| {
            let scores = r
                .data
                .get("score_history")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_f64)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (r.node_id, scores)
        })
        .collect();

    let mut count = 0u64;

    for sr in &salience_results {
        let current_score = sr
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        // Build score history: previous scores + current
        let mut scores = history
            .get(&sr.node_id)
            .cloned()
            .unwrap_or_default();
        scores.push(current_score);

        // Keep last 10 snapshots
        if scores.len() > 10 {
            let start_idx = scores.len() - 10;
            scores = scores[start_idx..].to_vec();
        }

        // Compute trend via linear regression over history
        let (slope, trend) = if scores.len() >= 2 {
            let points: Vec<(f64, f64)> = scores
                .iter()
                .enumerate()
                .map(|(i, &s)| (i as f64, s))
                .collect();
            let slope = simple_slope(&points);
            let trend = classify_trend(slope);
            (slope, trend)
        } else {
            (0.0, "Stable")
        };

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id: sr.node_id,
            kind: AnalysisKind::CentralityTrend,
            data: serde_json::json!({
                "trend": trend,
                "slope": (slope * 10000.0).round() / 10000.0,
                "current_score": (current_score * 10000.0).round() / 10000.0,
                "score_history": scores,
                "snapshots": scores.len(),
            }),
            input_hash: 0,
            computed_at: now,
        };
        store.store_analysis(&result).await?;
        count += 1;
    }

    Ok(count)
}

fn classify_trend(slope: f64) -> &'static str {
    if slope > 0.01 {
        "Rising"
    } else if slope < -0.01 {
        "Falling"
    } else {
        "Stable"
    }
}

/// Simple linear regression slope (least squares).
fn simple_slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    if n < 2.0 {
        return 0.0;
    }

    let sum_x: f64 = points.iter().map(|(x, _)| x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| y).sum();
    let dot_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_xx: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        return 0.0;
    }

    (n * dot_xy - sum_x * sum_y) / denom
}

// ── Architectural Drift ────────────────────────────────────────────

/// Measure cross-community coupling as a ratio of cross-community edges to total.
async fn compute_architectural_drift(
    store: &dyn HomerStore,
    now: chrono::DateTime<Utc>,
) -> crate::error::Result<u64> {
    // Load community assignments
    let community_results = store
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await?;

    if community_results.is_empty() {
        return Ok(0);
    }

    let node_community: HashMap<NodeId, u32> = community_results
        .iter()
        .filter_map(|r| {
            let comm = r
                .data
                .get("community_id")
                .and_then(serde_json::Value::as_u64)
                .map(|c| c as u32)?;
            Some((r.node_id, comm))
        })
        .collect();

    // Count import edges: total vs cross-community
    let import_edges = store.get_edges_by_kind(HyperedgeKind::Imports).await?;
    let mut total_edges = 0u64;
    let mut cross_edges = 0u64;
    let mut new_cross: Vec<(NodeId, NodeId)> = Vec::new();

    for edge in &import_edges {
        let src = edge.members.iter().find(|m| m.role == "source");
        let tgt = edge.members.iter().find(|m| m.role == "target");
        let Some((s, t)) = src.zip(tgt) else {
            continue;
        };

        total_edges += 1;
        let src_comm = node_community.get(&s.node_id);
        let tgt_comm = node_community.get(&t.node_id);

        if let (Some(sc), Some(tc)) = (src_comm, tgt_comm) {
            if sc != tc {
                cross_edges += 1;
                new_cross.push((s.node_id, t.node_id));
            }
        }
    }

    if total_edges == 0 {
        return Ok(0);
    }

    let coupling_ratio = cross_edges as f64 / total_edges as f64;

    // Load previous drift result for trend
    let existing_drift = store
        .get_analyses_by_kind(AnalysisKind::ArchitecturalDrift)
        .await?;

    let mut ratio_history: Vec<f64> = existing_drift
        .first()
        .and_then(|r| r.data.get("coupling_ratio_history"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_f64)
                .collect()
        })
        .unwrap_or_default();

    ratio_history.push(coupling_ratio);
    if ratio_history.len() > 10 {
        let start_idx = ratio_history.len() - 10;
        ratio_history = ratio_history[start_idx..].to_vec();
    }

    let drift_trend = if ratio_history.len() >= 2 {
        let points: Vec<(f64, f64)> = ratio_history
            .iter()
            .enumerate()
            .map(|(i, &r)| (i as f64, r))
            .collect();
        classify_trend(simple_slope(&points))
    } else {
        "Stable"
    };

    // Store as a single architectural drift result.
    // Attach to the first node with a community assignment (project-level metric).
    let anchor_node = community_results
        .first()
        .map_or(NodeId(1), |r| r.node_id);

    let result = AnalysisResult {
        id: AnalysisResultId(0),
        node_id: anchor_node,
        kind: AnalysisKind::ArchitecturalDrift,
        data: serde_json::json!({
            "coupling_ratio": (coupling_ratio * 10000.0).round() / 10000.0,
            "cross_community_edges": cross_edges,
            "total_edges": total_edges,
            "trend": drift_trend,
            "coupling_ratio_history": ratio_history,
            "new_cross_boundary_count": new_cross.len(),
        }),
        input_hash: 0,
        computed_at: now,
    };
    store.store_analysis(&result).await?;

    Ok(1)
}

// ── Enhanced Stability ─────────────────────────────────────────────

/// Enhance stability classification with `Declining` class from centrality trends.
async fn enhance_stability(
    store: &dyn HomerStore,
    now: chrono::DateTime<Utc>,
) -> crate::error::Result<u64> {
    let trend_results = store
        .get_analyses_by_kind(AnalysisKind::CentralityTrend)
        .await?;

    let stability_results = store
        .get_analyses_by_kind(AnalysisKind::StabilityClassification)
        .await?;

    if stability_results.is_empty() || trend_results.is_empty() {
        return Ok(0);
    }

    // Build trend lookup
    let trends: HashMap<NodeId, &str> = trend_results
        .iter()
        .filter_map(|r| {
            let trend = r
                .data
                .get("trend")
                .and_then(serde_json::Value::as_str)?;
            Some((r.node_id, trend))
        })
        .collect();

    let mut count = 0u64;

    for sr in &stability_results {
        let current_class = sr
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ReliableBackground");

        let trend = trends.get(&sr.node_id).copied().unwrap_or("Stable");

        // Upgrade to Declining if centrality is falling and not already ActiveCritical/Volatile
        let enhanced_class = if trend == "Falling"
            && !matches!(current_class, "ActiveCritical" | "Volatile")
        {
            "Declining"
        } else {
            current_class
        };

        // Only update if classification changed
        if enhanced_class != current_class {
            let centrality = sr
                .data
                .get("centrality")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let churn = sr
                .data
                .get("churn")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);

            let result = AnalysisResult {
                id: AnalysisResultId(0),
                node_id: sr.node_id,
                kind: AnalysisKind::StabilityClassification,
                data: serde_json::json!({
                    "classification": enhanced_class,
                    "centrality": centrality,
                    "churn": churn,
                    "centrality_trend": trend,
                }),
                input_hash: 0,
                computed_at: now,
            };
            store.store_analysis(&result).await?;
            count += 1;
        }
    }

    Ok(count)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HomerConfig;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{
        Hyperedge, HyperedgeId, HyperedgeMember, Node, NodeKind,
    };
    use std::collections::HashMap;

    #[test]
    fn trend_classification() {
        assert_eq!(classify_trend(0.05), "Rising");
        assert_eq!(classify_trend(-0.05), "Falling");
        assert_eq!(classify_trend(0.005), "Stable");
        assert_eq!(classify_trend(0.0), "Stable");
    }

    #[test]
    fn simple_slope_linear() {
        let points = vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)];
        let slope = simple_slope(&points);
        assert!((slope - 1.0).abs() < 0.001);
    }

    #[test]
    fn simple_slope_flat() {
        let points = vec![(0.0, 5.0), (1.0, 5.0), (2.0, 5.0)];
        let slope = simple_slope(&points);
        assert!(slope.abs() < 0.001);
    }

    async fn setup_temporal_data(store: &SqliteStore) {
        let now = Utc::now();

        // Create 4 module nodes
        let modules = ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"];
        for name in modules {
            store
                .upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::Module,
                    name: name.to_string(),
                    content_hash: None,
                    last_extracted: now,
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();
        }

        // Import edges: a→b (same community), a→c (cross), c→d (same)
        let edges = [
            ("src/a.rs", "src/b.rs"),
            ("src/a.rs", "src/c.rs"),
            ("src/c.rs", "src/d.rs"),
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
            store
                .upsert_hyperedge(&Hyperedge {
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
                    confidence: 1.0,
                    last_updated: now,
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();
        }

        // Run centrality + community first
        let centrality = crate::analyze::centrality::CentralityAnalyzer::default();
        centrality
            .analyze(store, &HomerConfig::default())
            .await
            .unwrap();

        let community = crate::analyze::community::CommunityAnalyzer::default();
        community
            .analyze(store, &HomerConfig::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn temporal_produces_trends() {
        let store = SqliteStore::in_memory().unwrap();
        setup_temporal_data(&store).await;

        let analyzer = TemporalAnalyzer;
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        assert!(stats.results_stored > 0, "Should produce temporal results");

        // Check centrality trends exist
        let trends = store
            .get_analyses_by_kind(AnalysisKind::CentralityTrend)
            .await
            .unwrap();
        assert!(!trends.is_empty(), "Should produce centrality trends");

        for t in &trends {
            let trend = t
                .data
                .get("trend")
                .and_then(serde_json::Value::as_str)
                .unwrap();
            assert!(
                ["Rising", "Stable", "Falling"].contains(&trend),
                "Invalid trend: {trend}"
            );
            assert!(
                t.data.get("score_history").is_some(),
                "Should have score_history"
            );
        }
    }

    #[tokio::test]
    async fn temporal_architectural_drift() {
        let store = SqliteStore::in_memory().unwrap();
        setup_temporal_data(&store).await;

        let analyzer = TemporalAnalyzer;
        let config = HomerConfig::default();
        analyzer.analyze(&store, &config).await.unwrap();

        // Check architectural drift
        let drift = store
            .get_analyses_by_kind(AnalysisKind::ArchitecturalDrift)
            .await
            .unwrap();

        // If communities were detected, should have drift analysis
        let community_results = store
            .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
            .await
            .unwrap();

        if !community_results.is_empty() {
            assert!(!drift.is_empty(), "Should produce drift analysis");
            let d = &drift[0];
            assert!(d.data.get("coupling_ratio").is_some());
            assert!(d.data.get("trend").is_some());
        }
    }

    #[tokio::test]
    async fn temporal_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let analyzer = TemporalAnalyzer;
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();
        assert_eq!(stats.results_stored, 0);
    }
}
