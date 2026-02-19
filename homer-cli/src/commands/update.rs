use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use tracing::info;

use homer_core::config::HomerConfig;
use homer_core::pipeline::HomerPipeline;
use homer_core::progress::IndicatifReporter;
use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::AnalysisKind;

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

    /// Force refresh of LLM-derived semantic analyses only
    #[arg(long)]
    pub force_semantic: bool,
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
    let db_path = super::resolve_db_path(&repo_path);
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

    // Clear only semantic (LLM-derived) analyses if --force-semantic
    if args.force_semantic {
        info!("Force-semantic mode: clearing LLM-derived analyses");
        let semantic_kinds = [
            AnalysisKind::SemanticSummary,
            AnalysisKind::DesignRationale,
            AnalysisKind::InvariantDescription,
        ];
        store
            .clear_analyses_by_kinds(&semantic_kinds)
            .await
            .context("Failed to clear semantic analyses")?;
    }

    let progress = IndicatifReporter::new();
    let pipeline = HomerPipeline::new(&repo_path);
    let result = pipeline
        .run_with_progress(&store, &config, &progress)
        .await
        .context("Pipeline execution failed")?;

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
