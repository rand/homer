use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::config::HomerConfig;
use homer_core::pipeline::HomerPipeline;
use homer_core::store::sqlite::SqliteStore;

#[derive(Args, Debug)]
pub struct RenderArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Comma-separated renderer names (default: use config)
    #[arg(long)]
    pub format: Option<String>,

    /// Run all renderers (overrides --format and config)
    #[arg(long)]
    pub all: bool,

    /// Comma-separated renderers to exclude (used with --all)
    #[arg(long)]
    pub exclude: Option<String>,
}

pub async fn run(args: RenderArgs) -> anyhow::Result<()> {
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

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Cannot read config: {}", config_path.display()))?;
    let config: HomerConfig = toml::from_str(&config_str)
        .with_context(|| format!("Cannot parse config: {}", config_path.display()))?;

    let db_path = homer_dir.join("homer.db");
    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    // Determine which renderers to run
    let renderer_names: Vec<String> = if args.all {
        HomerPipeline::ALL_RENDERER_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else if let Some(ref format) = args.format {
        format.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        config.renderers.enabled.clone()
    };

    // Apply --exclude filter
    let exclude: Vec<String> = args
        .exclude
        .as_deref()
        .map(|e| e.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let filtered: Vec<String> = renderer_names
        .into_iter()
        .filter(|n| !exclude.contains(n))
        .collect();

    if filtered.is_empty() {
        anyhow::bail!("No renderers selected. Use --all or --format to specify renderers.");
    }

    let name_refs: Vec<&str> = filtered.iter().map(String::as_str).collect();

    let pipeline = HomerPipeline::new(&repo_path);
    let result = pipeline
        .run_renderers(&store, &config, &name_refs)
        .await
        .context("Rendering failed")?;

    println!(
        "Rendered {} artifacts in {:.2?}",
        result.artifacts_written, result.duration
    );

    if !result.errors.is_empty() {
        println!();
        println!("  Warnings ({}):", result.errors.len());
        for error in &result.errors {
            println!("    - {error}");
        }
    }

    Ok(())
}
