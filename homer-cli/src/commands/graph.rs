use std::fmt::Write;
use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::AnalysisKind;

#[derive(Args, Debug)]
pub struct GraphArgs {
    /// Path to git repository (default: current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    /// Graph type: call, import, combined
    #[arg(long, default_value = "call", value_parser = ["call", "import", "combined"])]
    pub r#type: String,

    /// Metric to display: pagerank, betweenness, hits, salience
    #[arg(long, default_value = "salience")]
    pub metric: String,

    /// Show top N entities
    #[arg(long, default_value = "20")]
    pub top: usize,

    /// List all detected communities
    #[arg(long)]
    pub list_communities: bool,

    /// Show members of a specific community
    #[arg(long)]
    pub community: Option<u64>,

    /// Output format: text, json, dot, mermaid
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub async fn run(args: GraphArgs) -> anyhow::Result<()> {
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

    if args.list_communities {
        return list_communities(&db, &args.format).await;
    }

    if let Some(cid) = args.community {
        return show_community(&db, cid, &args.format).await;
    }

    show_metric_ranking(&db, &args.metric, &args.r#type, args.top, &args.format).await
}

// ── Metric Ranking ───────────────────────────────────────────────────

async fn show_metric_ranking(
    db: &SqliteStore,
    metric: &str,
    graph_type: &str,
    top: usize,
    format: &str,
) -> anyhow::Result<()> {
    let (kind, field) = match metric {
        "pagerank" => (AnalysisKind::PageRank, "pagerank"),
        "betweenness" => (AnalysisKind::BetweennessCentrality, "betweenness"),
        "hits" => (AnalysisKind::HITSScore, "authority_score"),
        "salience" => (AnalysisKind::CompositeSalience, "score"),
        other => {
            anyhow::bail!("Unknown metric: {other}. Use: pagerank, betweenness, hits, salience")
        }
    };

    let results = db.get_analyses_by_kind(kind).await?;

    let mut entries: Vec<(String, f64, String)> = Vec::new();
    for r in &results {
        let val = r
            .data
            .get(field)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let cls = r
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = db
            .get_node(r.node_id)
            .await?
            .map_or_else(|| format!("node:{}", r.node_id.0), |n| n.name);
        entries.push((name, val, cls));
    }

    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_entries: Vec<_> = entries.into_iter().take(top).collect();

    match format {
        "json" => print_ranking_json(&top_entries, metric, graph_type),
        "dot" => {
            print_ranking_dot(&top_entries, metric);
            Ok(())
        }
        "mermaid" => {
            print_ranking_mermaid(&top_entries, metric);
            Ok(())
        }
        _ => {
            print_ranking_text(&top_entries, metric, graph_type);
            Ok(())
        }
    }
}

fn print_ranking_text(entries: &[(String, f64, String)], metric: &str, graph_type: &str) {
    println!(
        "Top {} entities by {metric} ({graph_type} graph):",
        entries.len()
    );
    println!();
    println!("{:<4} {:<50} {:>10} Class", "#", "Entity", "Score");
    println!("{:-<80}", "");

    for (i, (name, val, cls)) in entries.iter().enumerate() {
        let display_name = if name.len() > 50 {
            format!("..{}", &name[name.len() - 48..])
        } else {
            name.clone()
        };
        println!("{:<4} {:<50} {:>10.4} {cls}", i + 1, display_name, val);
    }
}

fn print_ranking_json(
    entries: &[(String, f64, String)],
    metric: &str,
    graph_type: &str,
) -> anyhow::Result<()> {
    let json = serde_json::json!({
        "metric": metric,
        "graph_type": graph_type,
        "entries": entries.iter().enumerate().map(|(i, (name, val, cls))| {
            serde_json::json!({ "rank": i + 1, "name": name, "score": val, "classification": cls })
        }).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn print_ranking_dot(entries: &[(String, f64, String)], metric: &str) {
    let mut out = String::new();
    writeln!(out, "digraph {metric} {{").unwrap();
    writeln!(out, "  rankdir=LR;").unwrap();
    writeln!(out, "  node [shape=box];").unwrap();

    for (name, val, _cls) in entries {
        let label = name.rsplit("::").next().unwrap_or(name);
        let safe_id = name.replace(['/', '.', ':', '-'], "_");
        writeln!(out, "  {safe_id} [label=\"{label}\\n{val:.4}\"];").unwrap();
    }

    writeln!(out, "}}").unwrap();
    print!("{out}");
}

fn print_ranking_mermaid(entries: &[(String, f64, String)], metric: &str) {
    let mut out = String::new();
    writeln!(out, "graph LR").unwrap();
    writeln!(out, "  subgraph {metric}").unwrap();

    for (name, val, _cls) in entries {
        let label = name.rsplit("::").next().unwrap_or(name);
        let safe_id = name.replace(['/', '.', ':', '-'], "_");
        writeln!(out, "    {safe_id}[\"{label}<br/>{val:.4}\"]").unwrap();
    }

    writeln!(out, "  end").unwrap();
    print!("{out}");
}

// ── Community Listing ────────────────────────────────────────────────

async fn list_communities(db: &SqliteStore, format: &str) -> anyhow::Result<()> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await?;

    let mut communities: std::collections::HashMap<u64, Vec<String>> =
        std::collections::HashMap::new();
    for r in &results {
        let cid = r
            .data
            .get("community_id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let name = db
            .get_node(r.node_id)
            .await?
            .map_or_else(|| format!("node:{}", r.node_id.0), |n| n.name);
        communities.entry(cid).or_default().push(name);
    }

    if communities.is_empty() {
        println!("No communities detected. Run `homer init` or `homer update` first.");
        return Ok(());
    }

    if format == "json" {
        let json = serde_json::json!({
            "communities": communities.iter().map(|(id, members)| {
                serde_json::json!({ "id": id, "size": members.len(), "members": members })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let mut ids: Vec<_> = communities.keys().collect();
        ids.sort();

        println!("Detected {} communities:", communities.len());
        println!();
        for id in ids {
            let members = &communities[id];
            println!("  Community {id} ({} members):", members.len());
            for m in members.iter().take(10) {
                println!("    {m}");
            }
            if members.len() > 10 {
                println!("    ... and {} more", members.len() - 10);
            }
            println!();
        }
    }

    Ok(())
}

async fn show_community(db: &SqliteStore, cid: u64, format: &str) -> anyhow::Result<()> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await?;

    let mut members = Vec::new();
    for r in &results {
        let this_cid = r
            .data
            .get("community_id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(u64::MAX);
        if this_cid == cid {
            let aligned = r
                .data
                .get("directory_aligned")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let name = db
                .get_node(r.node_id)
                .await?
                .map_or_else(|| format!("node:{}", r.node_id.0), |n| n.name);
            members.push((name, aligned));
        }
    }

    if members.is_empty() {
        println!("Community {cid} not found.");
        return Ok(());
    }

    if format == "json" {
        let json = serde_json::json!({
            "community_id": cid,
            "size": members.len(),
            "members": members.iter().map(|(name, aligned)| {
                serde_json::json!({ "name": name, "directory_aligned": aligned })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("Community {cid} ({} members):", members.len());
        println!();
        for (name, aligned) in &members {
            let flag = if *aligned { "" } else { " [misaligned]" };
            println!("  {name}{flag}");
        }
    }

    Ok(())
}
