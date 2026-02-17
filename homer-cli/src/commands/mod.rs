pub mod init;
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
}

pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Init(args) => init::run(args).await,
        Command::Update(args) => update::run(args).await,
        Command::Status(args) => status::run(args).await,
    }
}
