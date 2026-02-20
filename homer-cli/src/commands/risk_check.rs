use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::config::HomerConfig;
use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, NodeFilter, NodeKind};

#[derive(Args, Debug)]
pub struct RiskCheckArgs {
    /// Path to git repository (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Risk score threshold (0.0-1.0) — fail if any file exceeds this
    #[arg(long, default_value = "0.7")]
    pub threshold: f64,

    /// Only check files matching this glob pattern
    #[arg(long)]
    pub filter: Option<String>,

    /// Output format: text or json
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub async fn run(args: RiskCheckArgs) -> anyhow::Result<()> {
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
    let _config: HomerConfig = toml::from_str(&config_str)
        .with_context(|| format!("Cannot parse config: {}", config_path.display()))?;

    let db_path = super::resolve_db_path(&repo_path);
    let db = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    let violations = find_violations(&db, &args).await?;
    print_results(&args, &violations);

    if violations.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "Risk check failed: {} files exceed threshold {:.1}",
            violations.len(),
            args.threshold
        )
    }
}

async fn find_violations(
    db: &SqliteStore,
    args: &RiskCheckArgs,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let node_filter = NodeFilter {
        kind: Some(NodeKind::File),
        name_contains: args.filter.clone(),
        ..Default::default()
    };

    let files = db
        .find_nodes(&node_filter)
        .await
        .context("Failed to query files")?;

    let mut violations = Vec::new();

    for file in &files {
        let salience_val = db
            .get_analysis(file.id, AnalysisKind::CompositeSalience)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.data.get("score").and_then(serde_json::Value::as_f64))
            .unwrap_or(0.0);

        let bus_factor = db
            .get_analysis(file.id, AnalysisKind::ContributorConcentration)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.data.get("bus_factor").and_then(serde_json::Value::as_u64))
            .unwrap_or(u64::MAX);

        let change_freq = db
            .get_analysis(file.id, AnalysisKind::ChangeFrequency)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.data.get("total").and_then(serde_json::Value::as_u64))
            .unwrap_or(0);

        let risk = compute_risk_score(salience_val, bus_factor, change_freq);

        if risk > args.threshold {
            violations.push(serde_json::json!({
                "file": file.name,
                "risk_score": risk,
                "salience": salience_val,
                "bus_factor": bus_factor,
                "change_frequency": change_freq,
            }));
        }
    }

    Ok(violations)
}

fn print_results(args: &RiskCheckArgs, violations: &[serde_json::Value]) {
    if args.format == "json" {
        let output = serde_json::json!({
            "threshold": args.threshold,
            "violations": violations.len(),
            "results": violations,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("self-constructed JSON is serializable")
        );
    } else if violations.is_empty() {
        println!(
            "Risk check PASS — no files exceed threshold {:.1}",
            args.threshold
        );
    } else {
        println!(
            "Risk check FAIL — {} files exceed threshold {:.1}:",
            violations.len(),
            args.threshold
        );
        println!();
        for v in violations {
            println!(
                "  {}: risk={:.2}, salience={:.2}, bus_factor={}, changes={}",
                v["file"].as_str().unwrap_or("?"),
                v["risk_score"].as_f64().unwrap_or(0.0),
                v["salience"].as_f64().unwrap_or(0.0),
                v["bus_factor"],
                v["change_frequency"],
            );
        }
    }
}

/// Compute a 0.0-1.0 risk score from component metrics.
fn compute_risk_score(salience: f64, bus_factor: u64, change_freq: u64) -> f64 {
    let mut val = 0.0;

    // Salience contributes 40% of risk
    val += salience * 0.4;

    // Low bus factor contributes 30%
    if bus_factor <= 1 {
        val += 0.3;
    } else if bus_factor <= 2 {
        val += 0.15;
    }

    // High change frequency contributes 30%
    if change_freq > 20 {
        val += 0.3;
    } else if change_freq > 10 {
        val += 0.2;
    } else if change_freq > 5 {
        val += 0.1;
    }

    val.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_score_low_for_safe_file() {
        assert!(compute_risk_score(0.1, 5, 2) < 0.2);
    }

    #[test]
    fn risk_score_high_for_risky_file() {
        assert!(compute_risk_score(0.9, 1, 25) > 0.8);
    }

    #[test]
    fn risk_score_capped_at_one() {
        assert!((compute_risk_score(1.0, 1, 100) - 1.0).abs() < f64::EPSILON);
    }
}
