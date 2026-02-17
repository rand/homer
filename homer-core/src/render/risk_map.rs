// Risk map renderer — produces `homer-risk.json` with risk and safe areas.
//
// Risk factors available in Phase 2:
// - high_centrality_low_tests: PageRank high but no test file detected
// - knowledge_silo: Bus factor == 1
// - volatile_critical: StabilityClassification == ActiveCritical
// - undocumented_critical: High centrality + no doc_comment

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;

use chrono::Utc;
use serde::Serialize;
use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, NodeFilter, NodeId, NodeKind};

use super::traits::Renderer;

#[derive(Debug)]
pub struct RiskMapRenderer;

#[async_trait::async_trait]
impl Renderer for RiskMapRenderer {
    fn name(&self) -> &'static str {
        "risk_map"
    }

    fn output_path(&self) -> &'static str {
        "homer-risk.json"
    }

    async fn render(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<String> {
        let risk_map = build_risk_map(store).await?;
        let json = serde_json::to_string_pretty(&risk_map).map_err(|e| {
            crate::error::HomerError::Render(crate::error::RenderError::Template(e.to_string()))
        })?;
        info!(
            risk_areas = risk_map.risk_areas.len(),
            safe_areas = risk_map.safe_areas.len(),
            "Risk map rendered"
        );
        Ok(json)
    }
}

// ── Risk Map Schema ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RiskMap {
    pub version: &'static str,
    pub generated_at: String,
    pub risk_areas: Vec<RiskArea>,
    pub safe_areas: Vec<SafeArea>,
}

#[derive(Debug, Serialize)]
pub struct RiskArea {
    pub path: String,
    pub risk_level: &'static str,
    pub risk_score: f64,
    pub reasons: Vec<RiskReason>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RiskReason {
    #[serde(rename = "type")]
    pub reason_type: &'static str,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centrality: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus_factor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_doc_comment: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct SafeArea {
    pub path: String,
    pub risk_level: &'static str,
    pub risk_score: f64,
    pub stability_class: String,
}

// ── Precomputed risk data ────────────────────────────────────────────

struct RiskData {
    salience: HashMap<NodeId, (f64, String, f64)>,
    bus: HashMap<NodeId, u64>,
    stability: HashMap<NodeId, String>,
    test_files: Vec<String>,
    file_has_docs: HashMap<String, bool>,
}

async fn load_risk_data(db: &dyn HomerStore) -> crate::error::Result<RiskData> {
    let salience_results = db.get_analyses_by_kind(AnalysisKind::CompositeSalience).await?;
    let salience: HashMap<_, _> = salience_results.iter().filter_map(|r| {
        let val = r.data.get("score")?.as_f64()?;
        let cls = r.data.get("classification").and_then(serde_json::Value::as_str).unwrap_or("Unknown");
        let pr = r.data.get("components").and_then(|c| c.get("pagerank")).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        Some((r.node_id, (val, cls.to_string(), pr)))
    }).collect();

    let bus_results = db.get_analyses_by_kind(AnalysisKind::ContributorConcentration).await?;
    let bus: HashMap<_, _> = bus_results.iter().filter_map(|r| {
        Some((r.node_id, r.data.get("bus_factor")?.as_u64()?))
    }).collect();

    let stab_results = db.get_analyses_by_kind(AnalysisKind::StabilityClassification).await?;
    let stability: HashMap<_, _> = stab_results.iter().filter_map(|r| {
        let cls = r.data.get("classification").and_then(serde_json::Value::as_str)?;
        Some((r.node_id, cls.to_string()))
    }).collect();

    let files = db.find_nodes(&NodeFilter { kind: Some(NodeKind::File), ..Default::default() }).await?;
    let test_files: Vec<_> = files.iter().filter(|f| {
        let name = f.name.to_lowercase();
        name.contains("test") || name.contains("spec") || name.ends_with("_test.go")
    }).map(|f| f.name.clone()).collect();

    let functions = db.find_nodes(&NodeFilter { kind: Some(NodeKind::Function), ..Default::default() }).await?;
    let mut file_has_docs: HashMap<String, bool> = HashMap::new();
    for func in &functions {
        if let Some(fp) = func.metadata.get("file").and_then(|v| v.as_str()) {
            if func.metadata.contains_key("doc_comment") {
                file_has_docs.insert(fp.to_string(), true);
            } else {
                file_has_docs.entry(fp.to_string()).or_insert(false);
            }
        }
    }

    Ok(RiskData { salience, bus, stability, test_files, file_has_docs })
}

// ── Builder ──────────────────────────────────────────────────────────

async fn build_risk_map(db: &dyn HomerStore) -> crate::error::Result<RiskMap> {
    let data = load_risk_data(db).await?;

    let files = db
        .find_nodes(&NodeFilter { kind: Some(NodeKind::File), ..Default::default() })
        .await?;

    let mut risk_areas = Vec::new();
    let mut safe_areas = Vec::new();

    for file in &files {
        let (reasons, risk_val) = assess_file_risk(file.id, &file.name, &data);

        if reasons.is_empty() {
            let stab_cls = data.stability.get(&file.id).cloned().unwrap_or_else(|| "Unknown".to_string());
            safe_areas.push(SafeArea {
                path: file.name.clone(),
                risk_level: classify_risk_level(risk_val),
                risk_score: risk_val,
                stability_class: stab_cls,
            });
        } else {
            let recommendations = generate_recommendations(&reasons);
            risk_areas.push(RiskArea {
                path: file.name.clone(),
                risk_level: classify_risk_level(risk_val),
                risk_score: risk_val,
                reasons,
                recommendations,
            });
        }
    }

    risk_areas.sort_by(|a, b| b.risk_score.partial_cmp(&a.risk_score).unwrap_or(std::cmp::Ordering::Equal));
    safe_areas.sort_by(|a, b| a.risk_score.partial_cmp(&b.risk_score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(RiskMap {
        version: "1.0",
        generated_at: Utc::now().to_rfc3339(),
        risk_areas,
        safe_areas,
    })
}

fn assess_file_risk(file_id: NodeId, file_name: &str, data: &RiskData) -> (Vec<RiskReason>, f64) {
    let mut reasons = Vec::new();
    let mut risk_val = 0.0_f64;

    let pagerank = data.salience.get(&file_id).map_or(0.0, |(_, _, pr)| *pr);
    let high_centrality = pagerank > 0.5;

    // Risk: high centrality but no associated test file
    if high_centrality && !has_associated_test(file_name, &data.test_files) {
        reasons.push(RiskReason {
            reason_type: "high_centrality_low_tests",
            description: format!("PageRank {pagerank:.2} but no test file detected"),
            centrality: Some(pagerank),
            bus_factor: None,
            has_doc_comment: None,
        });
        risk_val += 0.3;
    }

    // Risk: knowledge silo (bus factor <= 1)
    if let Some(&bf) = data.bus.get(&file_id) {
        if bf <= 1 {
            reasons.push(RiskReason {
                reason_type: "knowledge_silo",
                description: format!("Only {bf} contributor(s) in recent history"),
                centrality: None,
                bus_factor: Some(bf),
                has_doc_comment: None,
            });
            risk_val += 0.2;
        }
    }

    // Risk: volatile critical (ActiveCritical stability)
    if data.stability.get(&file_id).is_some_and(|s| s == "ActiveCritical") {
        reasons.push(RiskReason {
            reason_type: "volatile_critical",
            description: "High centrality with high churn".to_string(),
            centrality: Some(pagerank),
            bus_factor: None,
            has_doc_comment: None,
        });
        risk_val += 0.25;
    }

    // Risk: undocumented critical
    if high_centrality && !data.file_has_docs.get(file_name).copied().unwrap_or(false) {
        reasons.push(RiskReason {
            reason_type: "undocumented_critical",
            description: "High-centrality file with no doc comments".to_string(),
            centrality: Some(pagerank),
            bus_factor: None,
            has_doc_comment: Some(false),
        });
        risk_val += 0.15;
    }

    (reasons, risk_val.min(1.0))
}

fn has_associated_test(file_path: &str, test_files: &[String]) -> bool {
    let stem = file_path
        .rsplit('/')
        .next()
        .unwrap_or(file_path)
        .split('.')
        .next()
        .unwrap_or("");

    test_files.iter().any(|t| t.contains(stem))
}

fn classify_risk_level(val: f64) -> &'static str {
    if val >= 0.7 {
        "high"
    } else if val >= 0.4 {
        "medium"
    } else if val > 0.0 {
        "low"
    } else {
        "none"
    }
}

fn generate_recommendations(reasons: &[RiskReason]) -> Vec<String> {
    let mut recs = Vec::new();

    for reason in reasons {
        match reason.reason_type {
            "high_centrality_low_tests" => {
                recs.push("Consider adding test coverage before making changes".to_string());
                recs.push("Run full test suite after any modification".to_string());
            }
            "knowledge_silo" => {
                recs.push("Request review from the primary contributor".to_string());
                recs.push("Consider pair programming to spread knowledge".to_string());
            }
            "volatile_critical" => {
                recs.push(
                    "This file changes frequently and is structurally important — extra review recommended".to_string(),
                );
            }
            "undocumented_critical" => {
                recs.push("Add doc comments to public entities before making changes".to_string());
            }
            _ => {}
        }
    }

    recs.dedup();
    recs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{AnalysisResult, AnalysisResultId, Node, NodeId};
    use chrono::Utc;

    #[tokio::test]
    async fn renders_risk_map_json() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let file_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/core/engine.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("language".to_string(), serde_json::json!("rust"));
                    m
                },
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file_id,
                kind: AnalysisKind::CompositeSalience,
                data: serde_json::json!({
                    "score": 0.85,
                    "classification": "ActiveHotspot",
                    "components": { "pagerank": 0.9, "betweenness": 0.5, "change_frequency": 0.7 }
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file_id,
                kind: AnalysisKind::ContributorConcentration,
                data: serde_json::json!({ "bus_factor": 1, "top_contributor_share": 1.0 }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file_id,
                kind: AnalysisKind::StabilityClassification,
                data: serde_json::json!({ "classification": "ActiveCritical", "centrality": 0.9, "churn": 0.7 }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        let renderer = RiskMapRenderer;
        let config = HomerConfig::default();
        let json = renderer.render(&store, &config).await.unwrap();

        let risk_map: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(risk_map["version"], "1.0");

        let areas = risk_map["risk_areas"].as_array().unwrap();
        assert!(!areas.is_empty(), "Should have at least one risk area");

        let area = &areas[0];
        assert_eq!(area["path"], "src/core/engine.rs");
        assert!(area["risk_score"].as_f64().unwrap() > 0.3);

        let reason_types: Vec<_> = area["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["type"].as_str())
            .collect();
        assert!(reason_types.contains(&"knowledge_silo"), "Should detect knowledge silo: {reason_types:?}");
    }

    #[tokio::test]
    async fn safe_areas_for_low_risk_files() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/utils/helpers.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let risk_map = build_risk_map(&store).await.unwrap();
        assert!(!risk_map.safe_areas.is_empty(), "Should classify low-risk file as safe");
        assert_eq!(risk_map.safe_areas[0].path, "src/utils/helpers.rs");
    }

    #[test]
    fn risk_level_classification() {
        assert_eq!(classify_risk_level(0.9), "high");
        assert_eq!(classify_risk_level(0.5), "medium");
        assert_eq!(classify_risk_level(0.2), "low");
        assert_eq!(classify_risk_level(0.0), "none");
    }

    #[test]
    fn test_file_association() {
        let test_files = vec!["tests/test_engine.rs".to_string(), "src/main_test.go".to_string()];
        assert!(has_associated_test("src/engine.rs", &test_files));
        assert!(!has_associated_test("src/unknown.rs", &test_files));
    }
}
