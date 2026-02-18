pub mod diff;
pub mod graph;
pub mod init;
pub mod query;
pub mod render;
pub mod risk_check;
pub mod serve;
pub mod snapshot;
pub mod status;
pub mod update;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize Homer for a git repository (full extraction + analysis)
    Init(init::InitArgs),
    /// Incremental update — process new data since last run
    Update(update::UpdateArgs),
    /// Show current state of Homer's knowledge base
    Status(status::StatusArgs),
    /// Query the Homer knowledge base for an entity
    Query(query::QueryArgs),
    /// Explore graph analysis results
    Graph(graph::GraphArgs),
    /// Compare architectural state between two git refs
    Diff(diff::DiffArgs),
    /// Regenerate output artifacts without re-extraction
    Render(render::RenderArgs),
    /// Manage graph snapshots for temporal comparison
    Snapshot(snapshot::SnapshotArgs),
    /// Check risk thresholds for CI/CD gating
    RiskCheck(risk_check::RiskCheckArgs),
    /// Start MCP server for AI agent integration
    Serve(serve::ServeArgs),
}

pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Init(args) => init::run(args).await,
        Command::Update(args) => update::run(args).await,
        Command::Status(args) => status::run(args).await,
        Command::Query(args) => query::run(args).await,
        Command::Graph(args) => graph::run(args).await,
        Command::Diff(args) => diff::run(args).await,
        Command::Render(args) => render::run(args).await,
        Command::Snapshot(args) => snapshot::run(args).await,
        Command::RiskCheck(args) => risk_check::run(args).await,
        Command::Serve(args) => serve::run(args).await,
    }
}
