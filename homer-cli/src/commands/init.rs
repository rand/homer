use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use tracing::info;

use homer_core::config::{AnalysisDepth, HomerConfig};
use homer_core::pipeline::HomerPipeline;
use homer_core::progress::IndicatifReporter;
use homer_core::store::sqlite::SqliteStore;

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Analysis depth: shallow, standard, deep, full
    #[arg(long, default_value = "standard")]
    pub depth: String,

    /// Skip GitHub API extraction
    #[arg(long)]
    pub no_github: bool,

    /// Skip LLM-powered analysis
    #[arg(long)]
    pub no_llm: bool,

    /// Comma-separated list of languages to analyze
    #[arg(long)]
    pub languages: Option<String>,

    /// Custom database location
    #[arg(long)]
    pub db_path: Option<PathBuf>,
}

pub async fn run(args: InitArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let homer_dir = repo_path.join(".homer");
    let config_path = homer_dir.join("config.toml");

    // Check if already initialized
    if homer_dir.exists() && config_path.exists() {
        anyhow::bail!(
            "Homer is already initialized in {}. Use `homer update` to refresh.",
            repo_path.display()
        );
    }

    // Create .homer/ directory
    std::fs::create_dir_all(&homer_dir)
        .with_context(|| format!("Cannot create directory: {}", homer_dir.display()))?;

    // Build config from defaults + CLI overrides
    let mut config = HomerConfig::default();

    config.analysis.depth = match args.depth.as_str() {
        "shallow" => AnalysisDepth::Shallow,
        "standard" => AnalysisDepth::Standard,
        "deep" => AnalysisDepth::Deep,
        "full" => AnalysisDepth::Full,
        other => anyhow::bail!("Unknown depth '{other}'. Expected: shallow, standard, deep, full"),
    };

    if let Some(langs) = &args.languages {
        let lang_list: Vec<String> = langs.split(',').map(|s| s.trim().to_string()).collect();
        config.graph.languages = homer_core::config::LanguageConfig::Explicit(lang_list);
    }

    // Write config.toml
    let config_toml = toml::to_string_pretty(&config).context("Failed to serialize config")?;
    std::fs::write(&config_path, &config_toml)
        .with_context(|| format!("Cannot write config: {}", config_path.display()))?;

    info!(config_path = %config_path.display(), "Wrote config.toml");

    // Open database
    let db_path = args.db_path.unwrap_or_else(|| homer_dir.join("homer.db"));
    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    info!(db_path = %db_path.display(), "Opened database");

    // Run pipeline with progress reporting
    let progress = IndicatifReporter::new();
    let pipeline = HomerPipeline::new(&repo_path);
    let result = pipeline
        .run_with_progress(&store, &config, &progress)
        .await
        .context("Pipeline execution failed")?;

    // Report results
    println!("Homer initialized in {}", repo_path.display());
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

    println!();
    println!("  Database: {}", db_path.display());
    println!("  Config:   {}", config_path.display());

    // Check for AGENTS.md
    let agents_path = repo_path.join("AGENTS.md");
    if agents_path.exists() {
        println!("  Output:   {}", agents_path.display());
    }

    Ok(())
}
