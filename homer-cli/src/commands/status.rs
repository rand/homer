use clap::Args;

#[derive(Args, Debug)]
pub struct StatusArgs;

#[allow(clippy::unused_async)]
pub async fn run(_args: StatusArgs) -> anyhow::Result<()> {
    println!("homer status: not yet implemented");
    Ok(())
}
