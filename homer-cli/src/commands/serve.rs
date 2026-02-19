use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Transport type: stdio, sse (overrides config)
    #[arg(long)]
    pub transport: Option<String>,
    /// Host for SSE transport (overrides config)
    #[arg(long)]
    pub host: Option<String>,
    /// Port for SSE transport (overrides config)
    #[arg(long)]
    pub port: Option<u16>,
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
    let transport = args.transport.as_deref().unwrap_or(&config.mcp.transport);

    match transport {
        "stdio" => {
            homer_mcp::serve_stdio(&db_path)
                .await
                .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;
        }
        other => {
            anyhow::bail!("Unsupported transport: {other}. Supported: stdio");
        }
    }

    Ok(())
}
