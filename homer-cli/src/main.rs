use clap::Parser;

mod commands;

#[derive(Parser, Debug)]
#[command(
    name = "homer",
    version,
    about = "Mine git repositories for agentic development context"
)]
struct Cli {
    #[command(subcommand)]
    command: commands::Command,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    quiet: bool,
}

/// Classify an error into a spec-defined exit code.
///
/// Exit codes (from CLI.md spec):
///   0  — success
///   1  — general/unknown error
///   2  — configuration error
///   3  — repository not found / not initialized
///   4  — database error
///   5  — GitHub/forge API error (auth, rate limit)
///   6  — LLM API error
///   7  — render failed
///   8  — MCP server error
///   10 — partial success (some stages had non-fatal errors)
fn classify_exit_code(err: &anyhow::Error) -> i32 {
    let msg = format!("{err:#}");
    let lower = msg.to_lowercase();

    if lower.contains("not initialized") || lower.contains("cannot resolve path") {
        3 // repo not found
    } else if lower.contains("config") || lower.contains("unknown depth") {
        2 // config error
    } else if lower.contains("database")
        || lower.contains("sqlite")
        || lower.contains("cannot open database")
    {
        4 // database error
    } else if lower.contains("github api")
        || lower.contains("gitlab api")
        || lower.contains("rate limit")
        || lower.contains("api auth")
    {
        5 // forge API error
    } else if lower.contains("llm") || lower.contains("api_key") || lower.contains("llm api") {
        6 // LLM API error
    } else if lower.contains("rendering failed") || lower.contains("render") {
        7 // render failed
    } else if lower.contains("mcp") {
        8 // MCP server error
    } else {
        1 // general error
    }
}

fn main() {
    let cli = Cli::parse();

    // Initialize tracing based on verbosity
    let filter = match (cli.quiet, cli.verbose) {
        (true, _) => "error",
        (_, 0) => "warn",
        (_, 1) => "info",
        (_, 2) => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .init();

    // Run the selected command
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: Failed to create runtime: {e}");
            std::process::exit(1);
        }
    };

    match runtime.block_on(commands::run(cli.command)) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(classify_exit_code(&e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_not_initialized() {
        let err = anyhow::anyhow!("Homer is not initialized in /foo. Run `homer init` first.");
        assert_eq!(classify_exit_code(&err), 3);
    }

    #[test]
    fn exit_code_cannot_resolve() {
        let err = anyhow::anyhow!("Cannot resolve path: /nonexistent");
        assert_eq!(classify_exit_code(&err), 3);
    }

    #[test]
    fn exit_code_config() {
        let err = anyhow::anyhow!("Cannot parse config: bad toml");
        assert_eq!(classify_exit_code(&err), 2);
    }

    #[test]
    fn exit_code_database() {
        let err = anyhow::anyhow!("Cannot open database: /foo/.homer/homer.db");
        assert_eq!(classify_exit_code(&err), 4);
    }

    #[test]
    fn exit_code_github_api() {
        let err = anyhow::anyhow!("GitHub API error: rate limit exceeded");
        assert_eq!(classify_exit_code(&err), 5);
    }

    #[test]
    fn exit_code_llm_api() {
        let err = anyhow::anyhow!("LLM provider error: api_key not set");
        assert_eq!(classify_exit_code(&err), 6);
    }

    #[test]
    fn exit_code_general() {
        let err = anyhow::anyhow!("Something unexpected happened");
        assert_eq!(classify_exit_code(&err), 1);
    }
}
