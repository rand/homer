use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::config::HomerConfig;
use homer_core::pipeline::HomerPipeline;
use homer_core::render::traits::merge_with_preserve;
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

    /// Output directory (default: repo root)
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    /// Show what would be generated without writing files
    #[arg(long)]
    pub dry_run: bool,

    /// Show diff between existing artifacts and new render output
    /// (automatically merges with `<!-- homer:preserve -->` blocks)
    #[arg(long)]
    pub diff: bool,
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

    let db_path = super::resolve_db_path(&repo_path);
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

    let output_root = args.output_dir.as_deref().unwrap_or(&repo_path);

    if args.dry_run {
        println!("Dry run — would render to: {}", output_root.display());
        for name in &name_refs {
            println!("  - {name}");
        }
        return Ok(());
    }

    if args.diff {
        return show_diff(&store, &config, output_root, &name_refs).await;
    }

    let pipeline = HomerPipeline::new(output_root);
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

async fn show_diff(
    store: &SqliteStore,
    config: &HomerConfig,
    output_root: &std::path::Path,
    names: &[&str],
) -> anyhow::Result<()> {
    let mut any_diff = false;

    for name in names {
        let Some(renderer) = HomerPipeline::build_renderer(name) else {
            eprintln!("Unknown renderer: {name}");
            continue;
        };

        let new_content = renderer
            .render(store, config)
            .await
            .with_context(|| format!("Rendering {name} failed"))?;

        let output_path = output_root.join(renderer.output_path());
        let existing = if output_path.exists() {
            std::fs::read_to_string(&output_path)
                .with_context(|| format!("Cannot read {}", output_path.display()))?
        } else {
            String::new()
        };

        // Apply merge if the file exists (to match what write() would produce)
        let final_content = if existing.is_empty() {
            new_content
        } else {
            merge_with_preserve(&existing, &new_content)
        };

        if final_content == existing {
            println!("--- {}: no changes", renderer.output_path());
            continue;
        }

        any_diff = true;
        println!("--- a/{}", renderer.output_path());
        println!("+++ b/{}", renderer.output_path());

        // Simple line-by-line diff
        let old_lines: Vec<&str> = existing.lines().collect();
        let new_lines: Vec<&str> = final_content.lines().collect();
        print_simple_diff(&old_lines, &new_lines);
        println!();
    }

    if !any_diff {
        println!("No differences found.");
    }

    Ok(())
}

fn print_simple_diff(old: &[&str], new: &[&str]) {
    let max = old.len().max(new.len());
    let mut i = 0;
    while i < max {
        let old_line = old.get(i).copied();
        let new_line = new.get(i).copied();
        match (old_line, new_line) {
            (Some(o), Some(n)) if o == n => {}
            (Some(o), Some(n)) => {
                println!("-{o}");
                println!("+{n}");
            }
            (Some(o), None) => println!("-{o}"),
            (None, Some(n)) => println!("+{n}"),
            (None, None) => {}
        }
        i += 1;
    }
}
