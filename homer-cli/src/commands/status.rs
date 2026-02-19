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

    let db_path = super::resolve_db_path(&repo_path);
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
    let git_checkpoint = store.get_checkpoint("git_last_sha").await?;
    let graph_checkpoint = store.get_checkpoint("graph_last_sha").await?;

    println!("  Checkpoints:");
    match &git_checkpoint {
        Some(sha) => println!("    git_last_sha:   {}", &sha[..sha.len().min(12)]),
        None => println!("    git_last_sha:   (none)"),
    }
    match &graph_checkpoint {
        Some(sha) => println!("    graph_last_sha: {}", &sha[..sha.len().min(12)]),
        None => println!("    graph_last_sha: (none)"),
    }

    // Pending work
    let pending_commits = count_pending_commits(&repo_path, git_checkpoint.as_ref());
    if pending_commits > 0 {
        println!();
        println!("  Pending work:");
        println!(
            "    {pending_commits} new commit{} since last update",
            if pending_commits == 1 { "" } else { "s" }
        );
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

/// Count commits between the last-processed SHA and HEAD.
fn count_pending_commits(repo_path: &std::path::Path, checkpoint: Option<&String>) -> usize {
    let Ok(repo) = gix::open(repo_path) else {
        return 0;
    };
    let Ok(head) = repo.head_commit() else {
        return 0;
    };

    let Some(last_sha) = checkpoint else {
        // Never processed — count all commits (capped for display)
        return count_ancestors(&head, None);
    };

    let head_str = head.id().to_string();
    if head_str == *last_sha {
        return 0;
    }

    let Ok(stop_id) = repo.rev_parse_single(last_sha.as_str()) else {
        return 0;
    };

    count_ancestors(&head, Some(stop_id.detach()))
}

fn count_ancestors(commit: &gix::Commit<'_>, stop_at: Option<gix::ObjectId>) -> usize {
    let mut count = 0usize;
    let Ok(ancestors) = commit.ancestors().all() else {
        return 0;
    };
    for info in ancestors {
        let Ok(info) = info else { break };
        if let Some(ref stop) = stop_at {
            if info.id == *stop {
                break;
            }
        }
        count += 1;
        if count >= 9999 {
            break;
        }
    }
    count
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
