// Homer MCP server — exposes repository analysis as MCP tools for AI agents.
//
// Tools:
//   homer_query  — look up an entity by name (functions, types, files, modules)
//   homer_graph  — centrality metrics for top entities
//   homer_risk   — risk assessment for a file path

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_router};
use serde::Deserialize;
use tracing::info;

use homer_core::contracts::analysis_keys;
use homer_core::query;
use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, HyperedgeKind, NodeFilter, NodeKind};

// ── Tool parameter types ──────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// Entity name or substring to search for
    #[schemars(description = "Entity name or substring to search for")]
    pub entity: String,
    /// Kind filter: function, type, file, module, or all
    #[schemars(description = "Kind filter: function, type, file, module (omit for all)")]
    pub kind: Option<String>,
    /// Sections to include in the response
    #[schemars(
        description = "Sections to include: summary, metrics, callers, callees, history, co_changes (omit for default)"
    )]
    pub include: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphParams {
    /// Number of top entities to return (default: 10)
    #[schemars(description = "Number of top entities to return (default: 10)")]
    pub top: Option<u32>,
    /// Metric: pagerank, betweenness, hits, salience (default: salience)
    #[schemars(description = "Metric: pagerank, betweenness, hits, salience (default: salience)")]
    pub metric: Option<String>,
    /// File path prefix to scope results
    #[schemars(description = "File path prefix to scope results (e.g. 'src/core/')")]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RiskParams {
    /// File paths relative to repo root
    #[schemars(description = "File paths relative to repo root")]
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CoChangesParams {
    /// File path to find co-change partners for (omit for top global pairs)
    #[schemars(
        description = "File path to find co-change partners for (omit for top global pairs)"
    )]
    pub path: Option<String>,
    /// Maximum co-change pairs to return (default: 10)
    #[schemars(description = "Maximum co-change pairs to return (default: 10)")]
    pub top: Option<u32>,
    /// Minimum confidence threshold (default: 0.3)
    #[schemars(description = "Minimum confidence threshold (default: 0.3)")]
    pub min_confidence: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConventionsParams {
    /// Convention category filter
    #[schemars(
        description = "Convention type: naming, testing, error_handling, documentation, agent_rules (default: all)"
    )]
    pub category: Option<String>,
    /// Module path or empty for project-wide
    #[schemars(description = "Module path to scope conventions to, or omit for project-wide")]
    pub scope: Option<String>,
}

// ── Server struct ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HomerMcpServer {
    store: Arc<SqliteStore>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl HomerMcpServer {
    /// Create a new MCP server backed by the given Homer database.
    pub fn new(db_path: &std::path::Path) -> Result<Self, String> {
        let store =
            SqliteStore::open(db_path).map_err(|e| format!("Failed to open database: {e}"))?;
        Ok(Self {
            store: Arc::new(store),
            tool_router: Self::tool_router(),
        })
    }

    /// Create from an existing store (for testing).
    pub fn from_store(store: SqliteStore) -> Self {
        Self {
            store: Arc::new(store),
            tool_router: Self::tool_router(),
        }
    }
}

// ── Tool implementations ──────────────────────────────────────────

#[tool_router]
impl HomerMcpServer {
    #[tool(
        name = "homer_query",
        description = "Look up entities (functions, types, files, modules) by name in the Homer knowledge base. Returns metadata and salience data."
    )]
    async fn query(&self, Parameters(params): Parameters<QueryParams>) -> String {
        match self.do_query(params).await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        name = "homer_graph",
        description = "Get centrality metrics for top entities in the codebase. Identifies load-bearing code, structural bottlenecks, and architectural hubs."
    )]
    async fn graph(&self, Parameters(params): Parameters<GraphParams>) -> String {
        match self.do_graph(params).await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        name = "homer_risk",
        description = "Assess risk factors for a file path. Returns change frequency, bus factor, salience, community, and overall risk level. Use before modifying important files."
    )]
    async fn risk(&self, Parameters(params): Parameters<RiskParams>) -> String {
        match self.do_risk(params).await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        name = "homer_co_changes",
        description = "Find files that frequently change together. Use when planning modifications to understand ripple effects."
    )]
    async fn co_changes(&self, Parameters(params): Parameters<CoChangesParams>) -> String {
        match self.do_co_changes(params).await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        name = "homer_conventions",
        description = "Get project conventions (naming, testing, error handling, documentation). Use to understand and follow established patterns."
    )]
    async fn conventions(&self, Parameters(params): Parameters<ConventionsParams>) -> String {
        match self.do_conventions(params).await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }
}

impl ServerHandler for HomerMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Homer MCP server — codebase intelligence tools for AI agents. \
                 Use homer_query to look up entities, homer_graph for centrality metrics, \
                 homer_risk to assess modification risk, homer_co_changes to find files \
                 that change together, and homer_conventions to understand project patterns."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ── Tool logic (separated for testability) ────────────────────────

impl HomerMcpServer {
    async fn do_query(&self, params: QueryParams) -> Result<String, String> {
        let kind = params.kind.as_deref().and_then(query::parse_node_kind);
        let filter = NodeFilter {
            kind,
            name_contains: Some(params.entity.clone()),
            ..Default::default()
        };

        let nodes = self
            .store
            .find_nodes(&filter)
            .await
            .map_err(|e| format!("Store error: {e}"))?;

        if nodes.is_empty() {
            return serde_json::to_string_pretty(&serde_json::json!({
                "count": 0,
                "results": [],
                "note": format!("No entities found matching '{}'", params.entity),
            }))
            .map_err(|e| format!("JSON error: {e}"));
        }

        let include = params.include.as_deref().unwrap_or(&[]);
        let include_all = include.is_empty();

        let mut results = Vec::new();
        for node in nodes.iter().take(20) {
            let mut entry = serde_json::json!({
                "name": node.name,
                "kind": node.kind.as_str(),
            });

            if include_all || include.iter().any(|s| s == "summary") {
                entry["metadata"] = serde_json::json!(node.metadata);
            }

            if include_all || include.iter().any(|s| s == "metrics") {
                if let Ok(Some(sal)) = self
                    .store
                    .get_analysis(node.id, AnalysisKind::CompositeSalience)
                    .await
                {
                    entry["salience"] = sal.data;
                }
            }

            if include.iter().any(|s| s == "history") {
                if let Ok(Some(freq)) = self
                    .store
                    .get_analysis(node.id, AnalysisKind::ChangeFrequency)
                    .await
                {
                    entry["change_frequency"] = freq.data;
                }
            }

            if include.iter().any(|s| s == "callers" || s == "callees") {
                let (incoming, outgoing) = query::resolve_call_edges(&*self.store, node.id).await;
                if include.iter().any(|s| s == "callers") {
                    entry["callers"] = serde_json::json!(incoming);
                }
                if include.iter().any(|s| s == "callees") {
                    entry["callees"] = serde_json::json!(outgoing);
                }
            }

            if include.iter().any(|s| s == "co_changes") {
                entry["co_changes"] = serde_json::json!(
                    query::resolve_related_names(&*self.store, node.id, HyperedgeKind::CoChanges)
                        .await
                );
            }

            results.push(entry);
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "count": nodes.len(),
            "results": results,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn do_graph(&self, params: GraphParams) -> Result<String, String> {
        let top_n = params.top.unwrap_or(10) as usize;
        let metric = params.metric.as_deref().unwrap_or("salience");

        let analysis_kind = match metric {
            "pagerank" => AnalysisKind::PageRank,
            "betweenness" => AnalysisKind::BetweennessCentrality,
            "hits" => AnalysisKind::HITSScore,
            _ => AnalysisKind::CompositeSalience,
        };

        let results = self
            .store
            .get_analyses_by_kind(analysis_kind)
            .await
            .map_err(|e| format!("Store error: {e}"))?;

        let score_field = match metric {
            "pagerank" => analysis_keys::PAGERANK,
            "betweenness" => analysis_keys::BETWEENNESS,
            "hits" => analysis_keys::AUTHORITY_SCORE,
            _ => analysis_keys::SCORE,
        };

        let mut scored: Vec<_> = results
            .iter()
            .filter_map(|r| {
                let val = r.data.get(score_field)?.as_f64()?;
                Some((r.node_id, val, &r.data))
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Resolve names and apply scope filter BEFORE top-N truncation.
        let mut named: Vec<(String, f64, &serde_json::Value)> = Vec::new();
        for (node_id, val, data) in &scored {
            let name = query::resolve_name(&*self.store, *node_id).await;

            if let Some(ref scope) = params.scope {
                if !name.starts_with(scope.as_str()) {
                    continue;
                }
            }

            named.push((name, *val, *data));
        }

        let entries: Vec<_> = named
            .iter()
            .take(top_n)
            .map(|(name, val, data)| {
                serde_json::json!({
                    "name": name,
                    "score": val,
                    "data": data,
                })
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "metric": metric,
            "count": entries.len(),
            "results": entries,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }

    async fn do_risk(&self, params: RiskParams) -> Result<String, String> {
        let mut results = Vec::new();

        for path in &params.paths {
            let node = self
                .store
                .get_node_by_name(NodeKind::File, path)
                .await
                .map_err(|e| format!("Store error: {e}"))?;

            let Some(file_node) = node else {
                results.push(serde_json::json!({
                    "file": path,
                    "error": "not found in Homer database",
                }));
                continue;
            };

            let mut risk = serde_json::json!({ "file": path });

            let analyses = [
                (AnalysisKind::ChangeFrequency, "change_frequency"),
                (
                    AnalysisKind::ContributorConcentration,
                    "contributor_concentration",
                ),
                (AnalysisKind::CompositeSalience, "salience"),
                (AnalysisKind::CommunityAssignment, "community"),
                (AnalysisKind::StabilityClassification, "stability"),
            ];

            for (kind, key) in analyses {
                if let Ok(Some(result)) = self.store.get_analysis(file_node.id, kind).await {
                    risk[key] = result.data;
                }
            }

            risk["risk_level"] = serde_json::json!(compute_risk_level(&risk));
            results.push(risk);
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "count": results.len(),
            "results": results,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn do_co_changes(&self, params: CoChangesParams) -> Result<String, String> {
        let top_n = params.top.unwrap_or(10) as usize;
        let min_conf = params.min_confidence.unwrap_or(0.3);

        // Validate file path first (before querying edges)
        let target_node_id = if let Some(ref path) = params.path {
            let node = self
                .store
                .get_node_by_name(NodeKind::File, path)
                .await
                .map_err(|e| format!("Store error: {e}"))?;
            match node {
                Some(n) => Some(n.id),
                None => {
                    return serde_json::to_string_pretty(&serde_json::json!({
                        "count": 0,
                        "results": [],
                        "error": format!("File '{path}' not found in Homer database"),
                    }))
                    .map_err(|e| format!("JSON error: {e}"));
                }
            }
        } else {
            None
        };

        let edges = self
            .store
            .get_edges_by_kind(HyperedgeKind::CoChanges)
            .await
            .map_err(|e| format!("Store error: {e}"))?;

        if edges.is_empty() {
            return Ok(r#"{"count": 0, "results": [], "note": "No co-change data. Run `homer update` first."}"#.to_string());
        }

        let filtered: Vec<_> = if let Some(target) = target_node_id {
            edges
                .iter()
                .filter(|e| e.members.iter().any(|m| m.node_id == target))
                .collect()
        } else {
            edges.iter().collect()
        };

        let mut scored: Vec<_> = filtered
            .iter()
            .filter(|e| e.confidence >= min_conf)
            .map(|e| (e, e.confidence))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut results = Vec::new();
        for (edge, confidence) in scored.iter().take(top_n) {
            let mut member_names = Vec::new();
            for member in &edge.members {
                member_names.push(query::resolve_name(&*self.store, member.node_id).await);
            }

            let mut entry = serde_json::json!({
                "files": member_names,
                "confidence": confidence,
            });

            if let Some(co_occ) = edge.metadata.get("co_occurrences") {
                entry["co_occurrences"] = co_occ.clone();
            }
            if let Some(support) = edge.metadata.get("support") {
                entry["support"] = support.clone();
            }

            results.push(entry);
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "count": results.len(),
            "results": results,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }

    async fn do_conventions(&self, params: ConventionsParams) -> Result<String, String> {
        let kinds: Vec<(AnalysisKind, &str)> = match params.category.as_deref() {
            Some("naming") => vec![(AnalysisKind::NamingPattern, "naming")],
            Some("testing") => vec![(AnalysisKind::TestingPattern, "testing")],
            Some("error_handling") => {
                vec![(AnalysisKind::ErrorHandlingPattern, "error_handling")]
            }
            Some("documentation") => {
                vec![(AnalysisKind::DocumentationStylePattern, "documentation")]
            }
            Some("agent_rules") => vec![(AnalysisKind::AgentRuleValidation, "agent_rules")],
            Some(other) => {
                return serde_json::to_string_pretty(&serde_json::json!({
                    "count": 0,
                    "categories": {},
                    "error": format!(
                        "Unknown category '{other}'. Use: naming, testing, error_handling, documentation, agent_rules"
                    ),
                }))
                .map_err(|e| format!("JSON error: {e}"));
            }
            None => vec![
                (AnalysisKind::NamingPattern, "naming"),
                (AnalysisKind::TestingPattern, "testing"),
                (AnalysisKind::ErrorHandlingPattern, "error_handling"),
                (AnalysisKind::DocumentationStylePattern, "documentation"),
                (AnalysisKind::AgentRuleValidation, "agent_rules"),
            ],
        };

        let mut categories = serde_json::Map::new();

        for (kind, label) in &kinds {
            let analyses = self
                .store
                .get_analyses_by_kind(*kind)
                .await
                .map_err(|e| format!("Store error: {e}"))?;

            if analyses.is_empty() {
                continue;
            }

            // Aggregate per-file convention data into a project-wide summary
            let mut patterns: Vec<serde_json::Value> = Vec::new();
            for analysis in &analyses {
                let node_name = query::resolve_name(&*self.store, analysis.node_id).await;

                patterns.push(serde_json::json!({
                    "file": node_name,
                    "data": analysis.data,
                }));
            }

            categories.insert(
                (*label).to_string(),
                serde_json::json!({
                    "count": patterns.len(),
                    "patterns": patterns,
                }),
            );
        }

        if categories.is_empty() {
            return Ok(
                r#"{"count": 0, "categories": {}, "note": "No convention data. Run `homer update` first."}"#
                    .to_string(),
            );
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "count": categories.len(),
            "categories": categories,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }
}

// ── Helpers ───────────────────────────────────────────────────────

fn compute_risk_level(risk: &serde_json::Value) -> &'static str {
    let mut score = 0u32;

    if let Some(sal) = risk
        .get("salience")
        .and_then(|s| s.get("score"))
        .and_then(serde_json::Value::as_f64)
    {
        if sal > 0.7 {
            score += 3;
        } else if sal > 0.4 {
            score += 2;
        } else if sal > 0.2 {
            score += 1;
        }
    }

    if let Some(bf) = risk
        .get("contributor_concentration")
        .and_then(|c| c.get("bus_factor"))
        .and_then(serde_json::Value::as_u64)
    {
        if bf <= 1 {
            score += 2;
        }
    }

    if let Some(freq) = risk
        .get("change_frequency")
        .and_then(|f| f.get("total"))
        .and_then(serde_json::Value::as_u64)
    {
        if freq > 20 {
            score += 2;
        } else if freq > 10 {
            score += 1;
        }
    }

    match score {
        0..=1 => "low",
        2..=3 => "medium",
        4..=5 => "high",
        _ => "critical",
    }
}

/// Start the MCP server on stdio transport.
pub async fn serve_stdio(db_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let server = HomerMcpServer::new(db_path)?;
    info!("Starting Homer MCP server (stdio transport)");

    let transport = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}

/// Resolve the Homer database path from a repo path.
pub fn resolve_db_path(repo_path: &std::path::Path) -> Option<PathBuf> {
    let db = repo_path.join(".homer/homer.db");
    if db.exists() { Some(db) } else { None }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use homer_core::config::HomerConfig;
    use homer_core::contracts;
    use homer_core::pipeline::HomerPipeline;
    use homer_core::store::HomerStore;
    use homer_core::types::{
        AnalysisResult, AnalysisResultId, HyperedgeKind, Node, NodeFilter, NodeId, NodeKind,
    };
    use std::process::Command;

    #[test]
    fn risk_level_computation() {
        let empty = serde_json::json!({});
        assert_eq!(compute_risk_level(&empty), "low");

        let high = serde_json::json!({
            "salience": { "score": 0.8 },
            "contributor_concentration": { "bus_factor": 1 },
            "change_frequency": { "total": 25 },
        });
        assert_eq!(compute_risk_level(&high), "critical");

        let medium = serde_json::json!({
            "salience": { "score": 0.5 },
            "change_frequency": { "total": 5 },
        });
        assert_eq!(compute_risk_level(&medium), "medium");
    }

    #[tokio::test]
    async fn server_query_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_query(QueryParams {
                entity: "nonexistent".to_string(),
                kind: None,
                include: None,
            })
            .await
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert_eq!(json["count"], 0);
        assert!(
            json["note"]
                .as_str()
                .unwrap_or("")
                .contains("No entities found")
        );
    }

    #[tokio::test]
    async fn server_risk_missing_file() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_risk(RiskParams {
                paths: vec!["src/missing.rs".to_string()],
            })
            .await
            .unwrap();

        assert!(result.contains("not found"));
    }

    #[tokio::test]
    async fn server_co_changes_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_co_changes(CoChangesParams {
                path: None,
                top: None,
                min_confidence: None,
            })
            .await
            .unwrap();

        assert!(result.contains("\"count\": 0"));
    }

    #[tokio::test]
    async fn server_co_changes_missing_file() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_co_changes(CoChangesParams {
                path: Some("nonexistent.rs".to_string()),
                top: None,
                min_confidence: None,
            })
            .await
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert_eq!(json["count"], 0);
        assert!(json["error"].as_str().unwrap_or("").contains("not found"));
    }

    #[tokio::test]
    async fn server_conventions_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_conventions(ConventionsParams {
                category: None,
                scope: None,
            })
            .await
            .unwrap();

        assert!(result.contains("\"count\": 0"));
    }

    #[tokio::test]
    async fn server_conventions_unknown_category() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_conventions(ConventionsParams {
                category: Some("bogus".to_string()),
                scope: None,
            })
            .await
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert_eq!(json["count"], 0);
        assert!(
            json["error"]
                .as_str()
                .unwrap_or("")
                .contains("Unknown category")
        );
    }

    #[tokio::test]
    async fn server_graph_hits_uses_authority_score_field() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/main.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: std::collections::HashMap::new(),
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id,
                kind: AnalysisKind::HITSScore,
                data: serde_json::json!({
                    "hub_score": 0.2,
                    "authority_score": 0.9
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        let server = HomerMcpServer::from_store(store);
        let result = server
            .do_graph(GraphParams {
                top: Some(5),
                metric: Some("hits".to_string()),
                scope: None,
            })
            .await
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert_eq!(json["metric"], "hits");
        assert_eq!(json["count"], 1);
        assert_eq!(json["results"][0]["score"], 0.9);
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_DATE", "2025-01-15T10:00:00+00:00")
            .env("GIT_COMMITTER_DATE", "2025-01-15T10:00:00+00:00")
            .output()
            .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn make_repo_with_docs_and_imports() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        git(root, &["init"]);
        git(root, &["config", "user.email", "test@homer.dev"]);
        git(root, &["config", "user.name", "Test"]);

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"mcp-contract-test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub mod util;\n\npub fn greet() -> String {\n    util::format_name(\"world\")\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/util.rs"),
            "pub fn format_name(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("README.md"),
            "# MCP Contract Test\n\nSee [lib](src/lib.rs).\n",
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Initial commit"]);

        dir
    }

    async fn build_server_from_pipeline() -> HomerMcpServer {
        let repo = make_repo_with_docs_and_imports();
        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let pipeline = HomerPipeline::new(repo.path());
        pipeline.run(&store, &config).await.unwrap();
        HomerMcpServer::from_store(store)
    }

    #[tokio::test]
    async fn contract_pipeline_to_mcp_uses_canonical_fields() {
        let server = build_server_from_pipeline().await;

        let result = server
            .do_graph(GraphParams {
                top: Some(5),
                metric: Some("hits".to_string()),
                scope: None,
            })
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert_eq!(json["metric"], "hits");
        assert!(json["results"].is_array());

        // Documents edges produced by extractors must resolve with canonical/compat helpers.
        let doc_edges = server
            .store
            .get_edges_by_kind(HyperedgeKind::Documents)
            .await
            .unwrap();
        for edge in &doc_edges {
            assert!(
                contracts::find_document_pair(&edge.members).is_some(),
                "Documents edge must have contract-compatible roles"
            );
        }

        let docs = server
            .store
            .find_nodes(&NodeFilter {
                kind: Some(NodeKind::Document),
                ..Default::default()
            })
            .await
            .unwrap();
        for doc in &docs {
            assert!(
                doc.metadata
                    .contains_key(contracts::metadata_keys::DOC_TYPE),
                "Document nodes should use canonical doc_type metadata key"
            );
        }
    }
}
