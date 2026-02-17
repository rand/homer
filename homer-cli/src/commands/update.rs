use clap::Args;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Force full re-extraction (ignore checkpoints)
    #[arg(long)]
    pub force: bool,

    /// Force re-analysis (keep extraction, recompute all analysis)
    #[arg(long)]
    pub force_analysis: bool,
}

#[allow(clippy::unused_async)]
pub async fn run(_args: UpdateArgs) -> anyhow::Result<()> {
    println!("homer update: not yet implemented");
    Ok(())
}
