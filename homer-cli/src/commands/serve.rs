use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Transport type: stdio
    #[arg(long, default_value = "stdio")]
    pub transport: String,
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

    match args.transport.as_str() {
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
