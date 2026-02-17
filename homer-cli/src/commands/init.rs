use std::path::PathBuf;

use clap::Args;

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

#[allow(clippy::unused_async)]
pub async fn run(_args: InitArgs) -> anyhow::Result<()> {
    println!("homer init: not yet implemented");
    Ok(())
}
