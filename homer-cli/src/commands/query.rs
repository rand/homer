use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, HyperedgeKind, NodeFilter, NodeKind};

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// File path, function name, or qualified name to query
    pub entity: String,

    /// Path to git repository (default: current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    /// Output format: text, json
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub async fn run(args: QueryArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let db_path = repo_path.join(".homer/homer.db");
    if !db_path.exists() {
        anyhow::bail!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        );
    }

    let db = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    let node = find_entity(&db, &args.entity).await?;
    let Some(node) = node else {
        println!("No entity found matching: {}", args.entity);
        return Ok(());
    };

    if args.format == "json" {
        print_json(&db, &node).await?;
    } else {
        print_text_metrics(&db, &node).await?;
        print_text_edges(&db, &node).await?;
    }

    Ok(())
}

async fn find_entity(
    db: &SqliteStore,
    name: &str,
) -> anyhow::Result<Option<homer_core::types::Node>> {
    // Try exact match by kind
    for kind in [
        NodeKind::File,
        NodeKind::Function,
        NodeKind::Type,
        NodeKind::Module,
        NodeKind::Document,
    ] {
        if let Some(node) = db.get_node_by_name(kind, name).await? {
            return Ok(Some(node));
        }
    }

    // Try partial match
    for kind in [
        NodeKind::File,
        NodeKind::Function,
        NodeKind::Type,
        NodeKind::Module,
    ] {
        let nodes = db
            .find_nodes(&NodeFilter {
                kind: Some(kind),
                ..Default::default()
            })
            .await?;
        for node in nodes {
            if node.name.contains(name) || node.name.ends_with(name) {
                return Ok(Some(node));
            }
        }
    }

    Ok(None)
}

#[allow(clippy::too_many_lines)]
async fn print_text_metrics(
    db: &SqliteStore,
    node: &homer_core::types::Node,
) -> anyhow::Result<()> {
    println!("{}: {}", node.kind.as_str(), node.name);

    if let Some(lang) = node.metadata.get("language").and_then(|v| v.as_str()) {
        println!("Language: {lang}");
    }
    println!();
    println!("Metrics:");

    if let Some(sal) = db
        .get_analysis(node.id, AnalysisKind::CompositeSalience)
        .await?
    {
        let val = sal.data.get("score").and_then(serde_json::Value::as_f64);
        let cls = sal
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str);
        if let (Some(v), Some(c)) = (val, cls) {
            println!("  Composite Salience: {v:.2} ({c})");
        }
    }

    if let Some(pr) = db.get_analysis(node.id, AnalysisKind::PageRank).await? {
        let val = pr.data.get("pagerank").and_then(serde_json::Value::as_f64);
        let rank = pr.data.get("rank").and_then(serde_json::Value::as_u64);
        if let (Some(v), Some(r)) = (val, rank) {
            println!("  PageRank: {v:.4} (rank #{r})");
        }
    }

    if let Some(hits) = db.get_analysis(node.id, AnalysisKind::HITSScore).await? {
        let hub = hits
            .data
            .get("hub_score")
            .and_then(serde_json::Value::as_f64);
        let auth = hits
            .data
            .get("authority_score")
            .and_then(serde_json::Value::as_f64);
        let cls = hits
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str);
        if let (Some(h), Some(a)) = (hub, auth) {
            print!("  HITS: hub={h:.4}, authority={a:.4}");
            if let Some(c) = cls {
                print!(" ({c})");
            }
            println!();
        }
    }

    if let Some(freq) = db
        .get_analysis(node.id, AnalysisKind::ChangeFrequency)
        .await?
    {
        if let Some(t) = freq.data.get("total").and_then(serde_json::Value::as_u64) {
            println!("  Change Frequency: {t} total commits");
        }
    }

    if let Some(bus) = db
        .get_analysis(node.id, AnalysisKind::ContributorConcentration)
        .await?
    {
        if let Some(b) = bus
            .data
            .get("bus_factor")
            .and_then(serde_json::Value::as_u64)
        {
            println!("  Bus Factor: {b}");
        }
    }

    if let Some(stab) = db
        .get_analysis(node.id, AnalysisKind::StabilityClassification)
        .await?
    {
        if let Some(c) = stab
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
        {
            println!("  Stability: {c}");
        }
    }

    if let Some(comm) = db
        .get_analysis(node.id, AnalysisKind::CommunityAssignment)
        .await?
    {
        let cid = comm
            .data
            .get("community_id")
            .and_then(serde_json::Value::as_u64);
        let aligned = comm
            .data
            .get("directory_aligned")
            .and_then(serde_json::Value::as_bool);
        if let Some(id) = cid {
            print!("  Community: {id}");
            if aligned == Some(false) {
                print!(" (directory misaligned)");
            }
            println!();
        }
    }

    Ok(())
}

async fn print_text_edges(db: &SqliteStore, node: &homer_core::types::Node) -> anyhow::Result<()> {
    let edges = db.get_edges_involving(node.id).await?;

    let call_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == HyperedgeKind::Calls)
        .collect();
    if !call_edges.is_empty() {
        println!();
        println!("Call Graph:");
        for edge in &call_edges {
            let src = edge.members.iter().find(|m| m.role == "caller");
            let tgt = edge.members.iter().find(|m| m.role == "callee");
            if let (Some(s), Some(t)) = (src, tgt) {
                if s.node_id == node.id {
                    let nm = resolve_name(db, t.node_id).await;
                    println!("  -> calls: {nm}");
                } else {
                    let nm = resolve_name(db, s.node_id).await;
                    println!("  <- called by: {nm}");
                }
            }
        }
    }

    let import_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == HyperedgeKind::Imports)
        .collect();
    if !import_edges.is_empty() {
        println!();
        println!("Imports:");
        for edge in &import_edges {
            let src = edge.members.iter().find(|m| m.role == "source");
            let tgt = edge.members.iter().find(|m| m.role == "target");
            if let (Some(s), Some(t)) = (src, tgt) {
                if s.node_id == node.id {
                    let nm = resolve_name(db, t.node_id).await;
                    println!("  -> imports: {nm}");
                } else {
                    let nm = resolve_name(db, s.node_id).await;
                    println!("  <- imported by: {nm}");
                }
            }
        }
    }

    Ok(())
}

async fn print_json(db: &SqliteStore, node: &homer_core::types::Node) -> anyhow::Result<()> {
    let mut data = serde_json::json!({
        "kind": node.kind.as_str(),
        "name": node.name,
        "metadata": node.metadata,
    });

    let mut analyses = serde_json::Map::new();
    for kind in [
        AnalysisKind::CompositeSalience,
        AnalysisKind::PageRank,
        AnalysisKind::HITSScore,
        AnalysisKind::ChangeFrequency,
        AnalysisKind::ContributorConcentration,
        AnalysisKind::StabilityClassification,
        AnalysisKind::CommunityAssignment,
    ] {
        if let Some(result) = db.get_analysis(node.id, kind.clone()).await? {
            analyses.insert(format!("{kind:?}"), result.data);
        }
    }
    data["analyses"] = serde_json::Value::Object(analyses);

    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

async fn resolve_name(db: &SqliteStore, node_id: homer_core::types::NodeId) -> String {
    db.get_node(node_id)
        .await
        .ok()
        .flatten()
        .map_or_else(|| format!("node:{}", node_id.0), |n| n.name)
}
