use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

pub async fn run(args: StatusArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let homer_dir = repo_path.join(".homer");
    if !homer_dir.exists() {
        anyhow::bail!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        );
    }

    let db_path = homer_dir.join("homer.db");
    if !db_path.exists() {
        anyhow::bail!("Database not found: {}", db_path.display());
    }

    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    let stats = store.stats().await.context("Failed to read store stats")?;

    println!("Homer status for {}", repo_path.display());
    println!();
    println!("  Database: {}", db_path.display());

    // Database size
    if stats.db_size_bytes > 0 {
        let size = format_bytes(stats.db_size_bytes);
        println!("  Size:     {size}");
    }
    println!();

    // Node counts
    println!("  Nodes: {} total", stats.total_nodes);
    if !stats.nodes_by_kind.is_empty() {
        let mut kinds: Vec<_> = stats.nodes_by_kind.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1));
        for (kind, count) in &kinds {
            println!("    {kind:<20} {count:>6}");
        }
    }
    println!();

    // Edge counts
    println!("  Edges: {} total", stats.total_edges);
    if !stats.edges_by_kind.is_empty() {
        let mut kinds: Vec<_> = stats.edges_by_kind.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1));
        for (kind, count) in &kinds {
            println!("    {kind:<20} {count:>6}");
        }
    }
    println!();

    // Analysis results
    println!("  Analyses: {}", stats.total_analyses);
    println!();

    // Checkpoints
    let git_checkpoint = store.get_checkpoint("git_head").await?;
    let graph_checkpoint = store.get_checkpoint("graph_head_sha").await?;

    println!("  Checkpoints:");
    match &git_checkpoint {
        Some(sha) => println!("    git_head:       {}", &sha[..sha.len().min(12)]),
        None => println!("    git_head:       (none)"),
    }
    match &graph_checkpoint {
        Some(sha) => println!("    graph_head_sha: {}", &sha[..sha.len().min(12)]),
        None => println!("    graph_head_sha: (none)"),
    }

    // Check for AGENTS.md
    let agents_path = repo_path.join("AGENTS.md");
    println!();
    if agents_path.exists() {
        let meta = std::fs::metadata(&agents_path)?;
        println!(
            "  AGENTS.md: {} ({})",
            agents_path.display(),
            format_bytes(meta.len())
        );
    } else {
        println!("  AGENTS.md: not generated");
    }

    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
