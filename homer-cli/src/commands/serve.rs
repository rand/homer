use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Transport type (only stdio is supported)
    #[arg(long, value_parser = ["stdio"])]
    pub transport: Option<String>,
    /// Path to git repository (default: current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let db_path = homer_mcp::resolve_db_path(&repo_path).with_context(|| {
        format!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        )
    })?;

    // Load config for MCP defaults; fall back to defaults if missing.
    let config = super::load_config(&repo_path).unwrap_or_default();
    let transport = args
        .transport
        .as_deref()
        .unwrap_or(config.mcp.transport.as_str());

    match transport {
        "stdio" => {
            homer_mcp::serve_stdio(&db_path)
                .await
                .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;
        }
        other => {
            anyhow::bail!("Unsupported transport: {other}. Supported transport: stdio");
        }
    }

    Ok(())
}
