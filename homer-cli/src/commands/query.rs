use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::query;
use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, HyperedgeKind, NodeId};

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// File path, function name, or qualified name to query
    pub entity: String,

    /// Path to git repository (default: current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    /// Output format: text, json, markdown
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Sections to include (comma-separated): summary, metrics, callers, callees, history, all
    #[arg(long, default_value = "all")]
    pub include: String,

    /// Graph traversal depth for callers/callees sections
    #[arg(long, default_value_t = 1)]
    pub depth: u32,
}

#[allow(clippy::struct_excessive_bools)]
struct IncludeSections {
    summary: bool,
    metrics: bool,
    callers: bool,
    callees: bool,
    history: bool,
}

impl IncludeSections {
    fn parse(include: &str) -> Self {
        let parts: HashSet<&str> = include.split(',').map(str::trim).collect();
        let all = parts.contains("all");
        Self {
            summary: all || parts.contains("summary"),
            metrics: all || parts.contains("metrics"),
            callers: all || parts.contains("callers"),
            callees: all || parts.contains("callees"),
            history: all || parts.contains("history"),
        }
    }
}

pub async fn run(args: QueryArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let db_path = super::resolve_db_path(&repo_path);
    if !db_path.exists() {
        anyhow::bail!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        );
    }

    let db = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    let node = query::find_entity(&db, &args.entity).await?;
    let Some(node) = node else {
        println!("No entity found matching: {}", args.entity);
        return Ok(());
    };

    let sections = IncludeSections::parse(&args.include);

    match args.format.as_str() {
        "json" => print_json(&db, &node, &sections, args.depth).await?,
        "markdown" | "md" => print_markdown(&db, &node, &sections, args.depth).await?,
        _ => print_text(&db, &node, &sections, args.depth).await?,
    }

    Ok(())
}

// ── Text format ──────────────────────────────────────────────────

async fn print_text(
    db: &SqliteStore,
    node: &homer_core::types::Node,
    sections: &IncludeSections,
    depth: u32,
) -> anyhow::Result<()> {
    if sections.summary {
        emit_text_summary(node);
    }
    if sections.metrics {
        emit_text_metrics(db, node).await?;
    }
    if sections.callers {
        emit_text_neighbors(db, node.id, "Callers", "callee", "caller", depth).await?;
    }
    if sections.callees {
        emit_text_neighbors(db, node.id, "Callees", "caller", "callee", depth).await?;
    }
    if sections.history {
        emit_text_history(db, node).await?;
    }
    Ok(())
}

fn emit_text_summary(node: &homer_core::types::Node) {
    println!("{}: {}", node.kind.as_str(), node.name);
    if let Some(lang) = node.metadata.get("language").and_then(|v| v.as_str()) {
        println!("Language: {lang}");
    }
}

#[allow(clippy::too_many_lines)]
async fn emit_text_metrics(db: &SqliteStore, node: &homer_core::types::Node) -> anyhow::Result<()> {
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

async fn emit_text_neighbors(
    db: &SqliteStore,
    node_id: NodeId,
    label: &str,
    self_role: &str,
    neighbor_role: &str,
    depth: u32,
) -> anyhow::Result<()> {
    let neighbors =
        query::collect_neighbors_bfs(db, node_id, self_role, neighbor_role, depth).await?;
    if neighbors.is_empty() {
        return Ok(());
    }
    println!();
    println!("{label}:");
    for (d, name) in &neighbors {
        let indent = "  ".repeat(*d as usize);
        println!("{indent}{name}");
    }
    Ok(())
}

async fn emit_text_history(db: &SqliteStore, node: &homer_core::types::Node) -> anyhow::Result<()> {
    let edges = db.get_edges_involving(node.id).await?;
    let modifies: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == HyperedgeKind::Modifies)
        .collect();
    if modifies.is_empty() {
        return Ok(());
    }
    println!();
    println!("History:");
    for edge in modifies.iter().take(20) {
        // Modifies: commit (position 0) -> file (position 1)
        let commit_member = edge.members.iter().find(|m| m.position == 0);
        if let Some(cm) = commit_member {
            let name = query::resolve_name(db, cm.node_id).await;
            println!("  {name}");
        }
    }
    Ok(())
}

// ── Markdown format ─────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
async fn print_markdown(
    db: &SqliteStore,
    node: &homer_core::types::Node,
    sections: &IncludeSections,
    depth: u32,
) -> anyhow::Result<()> {
    let mut out = String::new();

    if sections.summary {
        let _ = writeln!(out, "# {} ({})\n", node.name, node.kind.as_str());
        if let Some(lang) = node.metadata.get("language").and_then(|v| v.as_str()) {
            let _ = writeln!(out, "**Language:** {lang}\n");
        }
    }

    if sections.metrics {
        let _ = writeln!(out, "## Metrics\n");
        emit_md_metrics(&mut out, db, node).await?;
    }

    if sections.callers {
        let callers = query::collect_neighbors_bfs(db, node.id, "callee", "caller", depth).await?;
        if !callers.is_empty() {
            let _ = writeln!(out, "## Callers\n");
            emit_md_neighbors(&mut out, &callers);
        }
    }

    if sections.callees {
        let callees = query::collect_neighbors_bfs(db, node.id, "caller", "callee", depth).await?;
        if !callees.is_empty() {
            let _ = writeln!(out, "## Callees\n");
            emit_md_neighbors(&mut out, &callees);
        }
    }

    if sections.history {
        emit_md_history(&mut out, db, node).await?;
    }

    print!("{out}");
    Ok(())
}

async fn emit_md_metrics(
    out: &mut String,
    db: &SqliteStore,
    node: &homer_core::types::Node,
) -> anyhow::Result<()> {
    let _ = writeln!(out, "| Metric | Value |");
    let _ = writeln!(out, "|--------|-------|");

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
            let _ = writeln!(out, "| Composite Salience | {v:.2} ({c}) |");
        }
    }

    if let Some(pr) = db.get_analysis(node.id, AnalysisKind::PageRank).await? {
        let val = pr.data.get("pagerank").and_then(serde_json::Value::as_f64);
        let rank = pr.data.get("rank").and_then(serde_json::Value::as_u64);
        if let (Some(v), Some(r)) = (val, rank) {
            let _ = writeln!(out, "| PageRank | {v:.4} (rank #{r}) |");
        }
    }

    if let Some(freq) = db
        .get_analysis(node.id, AnalysisKind::ChangeFrequency)
        .await?
    {
        if let Some(t) = freq.data.get("total").and_then(serde_json::Value::as_u64) {
            let _ = writeln!(out, "| Change Frequency | {t} commits |");
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
            let _ = writeln!(out, "| Bus Factor | {b} |");
        }
    }

    let _ = writeln!(out);
    Ok(())
}

fn emit_md_neighbors(out: &mut String, neighbors: &[(u32, String)]) {
    for (d, name) in neighbors {
        let indent = "  ".repeat(d.saturating_sub(1) as usize);
        let _ = writeln!(out, "{indent}- `{name}`");
    }
    let _ = writeln!(out);
}

async fn emit_md_history(
    out: &mut String,
    db: &SqliteStore,
    node: &homer_core::types::Node,
) -> anyhow::Result<()> {
    let edges = db.get_edges_involving(node.id).await?;
    let modifies: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == HyperedgeKind::Modifies)
        .collect();
    if modifies.is_empty() {
        return Ok(());
    }
    let _ = writeln!(out, "## History\n");
    for edge in modifies.iter().take(20) {
        let commit_member = edge.members.iter().find(|m| m.position == 0);
        if let Some(cm) = commit_member {
            let name = query::resolve_name(db, cm.node_id).await;
            let _ = writeln!(out, "- `{name}`");
        }
    }
    let _ = writeln!(out);
    Ok(())
}

// ── JSON format ─────────────────────────────────────────────────

async fn print_json(
    db: &SqliteStore,
    node: &homer_core::types::Node,
    sections: &IncludeSections,
    depth: u32,
) -> anyhow::Result<()> {
    let mut data = serde_json::json!({
        "kind": node.kind.as_str(),
        "name": node.name,
        "metadata": node.metadata,
    });

    if sections.metrics {
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
            if let Some(result) = db.get_analysis(node.id, kind).await? {
                analyses.insert(format!("{kind:?}"), result.data);
            }
        }
        data["analyses"] = serde_json::Value::Object(analyses);
    }

    if sections.callers {
        let callers = query::collect_neighbors_bfs(db, node.id, "callee", "caller", depth).await?;
        data["callers"] =
            serde_json::Value::Array(callers.into_iter().map(|(_, n)| n.into()).collect());
    }

    if sections.callees {
        let callees = query::collect_neighbors_bfs(db, node.id, "caller", "callee", depth).await?;
        data["callees"] =
            serde_json::Value::Array(callees.into_iter().map(|(_, n)| n.into()).collect());
    }

    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}
