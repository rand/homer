use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand};

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    #[command(subcommand)]
    pub action: SnapshotAction,
}

#[derive(Subcommand, Debug)]
pub enum SnapshotAction {
    /// Create a named snapshot of the current graph state
    Create {
        /// Label for the snapshot (e.g., "v1.0", "before-refactor")
        label: String,
    },
    /// List all snapshots
    List,
    /// Delete a snapshot by label
    Delete {
        /// Label of the snapshot to delete
        label: String,
    },
}

pub async fn run(args: SnapshotArgs) -> anyhow::Result<()> {
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
    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    match args.action {
        SnapshotAction::Create { label } => {
            let snap_id = store
                .create_snapshot(&label)
                .await
                .context("Failed to create snapshot")?;
            println!("Created snapshot '{label}' (id: {snap_id})");
        }
        SnapshotAction::List => {
            let snapshots = store
                .list_snapshots()
                .await
                .context("Failed to list snapshots")?;

            if snapshots.is_empty() {
                println!("No snapshots found.");
            } else {
                println!(
                    "{:<6} {:<20} {:<24} {:>8} {:>8}",
                    "ID", "LABEL", "CREATED", "NODES", "EDGES"
                );
                for snap in &snapshots {
                    println!(
                        "{:<6} {:<20} {:<24} {:>8} {:>8}",
                        snap.id,
                        snap.label,
                        snap.snapshot_at.format("%Y-%m-%d %H:%M:%S UTC"),
                        snap.node_count,
                        snap.edge_count,
                    );
                }
            }
        }
        SnapshotAction::Delete { label } => {
            let deleted = store
                .delete_snapshot(&label)
                .await
                .context("Failed to delete snapshot")?;
            if deleted {
                println!("Deleted snapshot '{label}'");
            } else {
                println!("Snapshot '{label}' not found");
            }
        }
    }

    Ok(())
}
