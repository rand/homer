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
use rmcp::{schemars, tool, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use tracing::info;

use homer_core::store::sqlite::SqliteStore;
use homer_core::store::HomerStore;
use homer_core::types::{AnalysisKind, NodeFilter, NodeKind};

// ── Tool parameter types ──────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// Entity name or substring to search for
    #[schemars(description = "Entity name or substring to search for")]
    pub name: String,
    /// Kind filter: function, type, file, module, or all
    #[schemars(description = "Kind filter: function, type, file, module (omit for all)")]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphParams {
    /// Number of top entities to return (default: 10)
    #[schemars(description = "Number of top entities to return (default: 10)")]
    pub top: Option<u32>,
    /// Metric: pagerank, betweenness, hits, salience (default: salience)
    #[schemars(description = "Metric: pagerank, betweenness, hits, salience (default: salience)")]
    pub metric: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RiskParams {
    /// File path relative to repo root
    #[schemars(description = "File path relative to repo root")]
    pub path: String,
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
}

impl ServerHandler for HomerMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Homer MCP server — codebase intelligence tools for AI agents. \
                 Use homer_query to look up entities, homer_graph for centrality metrics, \
                 and homer_risk to assess modification risk."
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
        let kind = params.kind.as_deref().and_then(parse_node_kind);
        let filter = NodeFilter {
            kind,
            name_contains: Some(params.name.clone()),
            ..Default::default()
        };

        let nodes = self
            .store
            .find_nodes(&filter)
            .await
            .map_err(|e| format!("Store error: {e}"))?;

        if nodes.is_empty() {
            return Ok(format!("No entities found matching '{}'", params.name));
        }

        let mut results = Vec::new();
        for node in nodes.iter().take(20) {
            let mut entry = serde_json::json!({
                "name": node.name,
                "kind": node.kind.as_str(),
                "metadata": node.metadata,
            });

            if let Ok(Some(sal)) = self
                .store
                .get_analysis(node.id, AnalysisKind::CompositeSalience)
                .await
            {
                entry["salience"] = sal.data;
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
            "pagerank" => "pagerank",
            "betweenness" => "betweenness",
            "hits" => "authority",
            _ => "score",
        };

        let mut scored: Vec<_> = results
            .iter()
            .filter_map(|r| {
                let val = r.data.get(score_field)?.as_f64()?;
                Some((r.node_id, val, &r.data))
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut entries = Vec::new();
        for (node_id, val, data) in scored.iter().take(top_n) {
            let name = self
                .store
                .get_node(*node_id)
                .await
                .ok()
                .flatten()
                .map_or_else(|| format!("node:{}", node_id.0), |n| n.name);
            entries.push(serde_json::json!({
                "name": name,
                "score": val,
                "data": data,
            }));
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "metric": metric,
            "count": entries.len(),
            "results": entries,
        }))
        .map_err(|e| format!("JSON error: {e}"))
    }

    async fn do_risk(&self, params: RiskParams) -> Result<String, String> {
        let node = self
            .store
            .get_node_by_name(NodeKind::File, &params.path)
            .await
            .map_err(|e| format!("Store error: {e}"))?;

        let Some(file_node) = node else {
            return Ok(format!("File '{}' not found in Homer database", params.path));
        };

        let mut risk = serde_json::json!({ "file": params.path });

        let analyses = [
            (AnalysisKind::ChangeFrequency, "change_frequency"),
            (AnalysisKind::ContributorConcentration, "contributor_concentration"),
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

        serde_json::to_string_pretty(&risk).map_err(|e| format!("JSON error: {e}"))
    }
}

// ── Helpers ───────────────────────────────────────────────────────

fn parse_node_kind(s: &str) -> Option<NodeKind> {
    match s.to_lowercase().as_str() {
        "function" | "fn" => Some(NodeKind::Function),
        "type" | "struct" | "class" => Some(NodeKind::Type),
        "file" => Some(NodeKind::File),
        "module" | "dir" | "directory" => Some(NodeKind::Module),
        "commit" => Some(NodeKind::Commit),
        "contributor" | "author" => Some(NodeKind::Contributor),
        "pr" | "pullrequest" => Some(NodeKind::PullRequest),
        "issue" => Some(NodeKind::Issue),
        "dep" | "dependency" => Some(NodeKind::ExternalDep),
        "document" | "doc" => Some(NodeKind::Document),
        _ => None,
    }
}

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
    let server = HomerMcpServer::new(db_path).map_err(|e| e.to_string())?;
    info!("Starting Homer MCP server (stdio transport)");

    let transport = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}

/// Resolve the Homer database path from a repo path.
pub fn resolve_db_path(repo_path: &std::path::Path) -> Option<PathBuf> {
    let db = repo_path.join(".homer/homer.db");
    if db.exists() {
        Some(db)
    } else {
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_node_kind_variants() {
        assert_eq!(parse_node_kind("function"), Some(NodeKind::Function));
        assert_eq!(parse_node_kind("fn"), Some(NodeKind::Function));
        assert_eq!(parse_node_kind("type"), Some(NodeKind::Type));
        assert_eq!(parse_node_kind("file"), Some(NodeKind::File));
        assert_eq!(parse_node_kind("module"), Some(NodeKind::Module));
        assert_eq!(parse_node_kind("unknown"), None);
        assert_eq!(parse_node_kind("all"), None);
    }

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
                name: "nonexistent".to_string(),
                kind: None,
            })
            .await
            .unwrap();

        assert!(result.contains("No entities found"));
    }

    #[tokio::test]
    async fn server_risk_missing_file() {
        let store = SqliteStore::in_memory().unwrap();
        let server = HomerMcpServer::from_store(store);

        let result = server
            .do_risk(RiskParams {
                path: "src/missing.rs".to_string(),
            })
            .await
            .unwrap();

        assert!(result.contains("not found"));
    }
}
