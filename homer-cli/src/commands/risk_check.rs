use std::collections::HashSet;
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

    /// Base ref to diff against HEAD (e.g. main, origin/main) — scope to changed files only
    #[arg(long)]
    pub diff: Option<String>,
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

    let diff_paths = if let Some(ref base_ref) = args.diff {
        Some(git_diff_file_list(&repo_path, base_ref)?)
    } else {
        None
    };

    let violations = find_violations(&db, &args, diff_paths.as_deref()).await?;
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
    diff_paths: Option<&[String]>,
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

    // When --diff is provided, only check files in the diff set
    let diff_set: Option<HashSet<&str>> =
        diff_paths.map(|paths| paths.iter().map(String::as_str).collect());

    let mut violations = Vec::new();

    for file in &files {
        if let Some(ref set) = diff_set {
            if !set.contains(file.name.as_str()) {
                continue;
            }
        }
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

/// Get the list of changed file paths between `base_ref` and HEAD.
fn git_diff_file_list(repo_path: &std::path::Path, base_ref: &str) -> anyhow::Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", &format!("{base_ref}...HEAD")])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
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

    #[tokio::test]
    async fn diff_filter_restricts_checked_files() {
        use homer_core::types::{AnalysisResult, AnalysisResultId, Node, NodeId};
        use std::collections::HashMap;

        let store = SqliteStore::in_memory().unwrap();

        // Create two files: one in the diff set, one not
        let file_a = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/risky.rs".to_string(),
                content_hash: None,
                last_extracted: chrono::Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let file_b = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/safe.rs".to_string(),
                content_hash: None,
                last_extracted: chrono::Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Give both files high salience
        for nid in [file_a, file_b] {
            store
                .store_analysis(&AnalysisResult {
                    id: AnalysisResultId(0),
                    node_id: nid,
                    kind: AnalysisKind::CompositeSalience,
                    data: serde_json::json!({"score": 0.9}),
                    input_hash: 0,
                    computed_at: chrono::Utc::now(),
                })
                .await
                .unwrap();
        }

        // Without diff: both should be checked
        let args_no_diff = RiskCheckArgs {
            path: ".".into(),
            threshold: 0.1,
            filter: None,
            format: "text".into(),
            diff: None,
        };
        let all = find_violations(&store, &args_no_diff, None).await.unwrap();
        assert_eq!(all.len(), 2, "Without diff, both files checked");

        // With diff: only src/risky.rs
        let diff_paths = vec!["src/risky.rs".to_string()];
        let filtered = find_violations(&store, &args_no_diff, Some(&diff_paths))
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1, "With diff, only diff files checked");
        assert_eq!(filtered[0]["file"], "src/risky.rs");
    }
}
