use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use homer_core::config::HomerConfig;
use homer_core::pipeline::HomerPipeline;
use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Force full re-extraction (ignore checkpoints)
    #[arg(long)]
    pub force: bool,

    /// Force re-analysis (keep extraction, recompute all analysis)
    #[arg(long)]
    pub force_analysis: bool,
}

pub async fn run(args: UpdateArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let homer_dir = repo_path.join(".homer");
    let config_path = homer_dir.join("config.toml");

    if !homer_dir.exists() || !config_path.exists() {
        anyhow::bail!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        );
    }

    // Load config
    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Cannot read config: {}", config_path.display()))?;
    let config: HomerConfig = toml::from_str(&config_str)
        .with_context(|| format!("Cannot parse config: {}", config_path.display()))?;

    // Open database
    let db_path = homer_dir.join("homer.db");
    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    // Clear checkpoints if --force
    if args.force {
        info!("Force mode: clearing all checkpoints");
        store
            .clear_checkpoints()
            .await
            .context("Failed to clear checkpoints")?;
    }

    // Clear analysis results if --force-analysis
    if args.force_analysis {
        info!("Force-analysis mode: clearing analysis results");
        store
            .clear_analyses()
            .await
            .context("Failed to clear analyses")?;
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    pb.set_message("Updating Homer...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    let pipeline = HomerPipeline::new(&repo_path);
    let result = pipeline
        .run(&store, &config)
        .await
        .context("Pipeline execution failed")?;

    pb.finish_and_clear();

    println!("Homer updated in {}", repo_path.display());
    println!();
    println!("  Nodes extracted: {}", result.extract_nodes);
    println!("  Edges created:   {}", result.extract_edges);
    println!("  Analyses run:    {}", result.analysis_results);
    println!("  Artifacts:       {}", result.artifacts_written);
    println!("  Duration:        {:.2?}", result.duration);

    if !result.errors.is_empty() {
        println!();
        println!("  Warnings ({}):", result.errors.len());
        for error in &result.errors {
            println!("    - {error}");
        }
    }

    Ok(())
}
