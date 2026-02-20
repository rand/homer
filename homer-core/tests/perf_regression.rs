use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use homer_core::analyze::behavioral::BehavioralAnalyzer;
use homer_core::analyze::centrality::CentralityAnalyzer;
use homer_core::analyze::traits::Analyzer;
use homer_core::config::HomerConfig;
use homer_core::extract::graph::GraphExtractor;
use homer_core::extract::structure::StructureExtractor;
use homer_core::extract::traits::Extractor;
use homer_core::pipeline::HomerPipeline;
use homer_core::store::sqlite::SqliteStore;

fn threshold_ms(var: &str, default_ms: u64) -> Duration {
    let ms = std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_ms);
    Duration::from_millis(ms)
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2025-01-15T10:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2025-01-15T10:00:00+00:00")
        .output()
        .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn synthetic_repo(file_count: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();

    git(root, &["init"]);
    git(root, &["config", "user.email", "test@homer.dev"]);
    git(root, &["config", "user.name", "Test"]);

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"perf-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    for i in 0..file_count {
        let path = root.join(format!("src/mod_{i}.rs"));
        std::fs::write(
            path,
            format!(
                "pub fn f_{i}(x: i32) -> i32 {{\n    x + {i}\n}}\n\npub fn call_{i}(x: i32) -> i32 {{\n    f_{i}(x)\n}}\n"
            ),
        )
        .unwrap();
    }

    std::fs::write(root.join("README.md"), "# Perf Fixture\n").unwrap();

    git(root, &["add", "."]);
    git(root, &["commit", "-m", "Initial fixture"]);

    dir
}

#[tokio::test]
#[ignore = "performance gate; run explicitly in CI/dev workflows"]
async fn perf_parse_path_under_threshold() {
    let repo = synthetic_repo(120);
    let store = SqliteStore::in_memory().unwrap();
    let config = HomerConfig::default();
    let structure = StructureExtractor::new(repo.path());
    let graph = GraphExtractor::new(repo.path());

    let t0 = Instant::now();
    structure.extract(&store, &config).await.unwrap();
    let structure_elapsed = t0.elapsed();

    let t1 = Instant::now();
    graph.extract(&store, &config).await.unwrap();
    let graph_elapsed = t1.elapsed();

    assert!(
        structure_elapsed <= threshold_ms("HOMER_PERF_STRUCTURE_MS", 5000),
        "structure extraction exceeded threshold: {structure_elapsed:?}"
    );
    assert!(
        graph_elapsed <= threshold_ms("HOMER_PERF_PARSE_MS", 10000),
        "graph parsing/extraction exceeded threshold: {graph_elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "performance gate; run explicitly in CI/dev workflows"]
async fn perf_centrality_under_threshold() {
    let repo = synthetic_repo(120);
    let store = SqliteStore::in_memory().unwrap();
    let config = HomerConfig::default();
    let structure = StructureExtractor::new(repo.path());
    let graph = GraphExtractor::new(repo.path());

    structure.extract(&store, &config).await.unwrap();
    graph.extract(&store, &config).await.unwrap();

    // Ensure prerequisite behavioral metrics exist for composite salience.
    BehavioralAnalyzer.analyze(&store, &config).await.unwrap();

    let t0 = Instant::now();
    CentralityAnalyzer::default()
        .analyze(&store, &config)
        .await
        .unwrap();
    let elapsed = t0.elapsed();

    assert!(
        elapsed <= threshold_ms("HOMER_PERF_CENTRALITY_MS", 10000),
        "centrality analysis exceeded threshold: {elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "performance gate; run explicitly in CI/dev workflows"]
async fn perf_incremental_noop_under_threshold() {
    let repo = synthetic_repo(120);
    let store = SqliteStore::in_memory().unwrap();
    let config = HomerConfig::default();
    let pipeline = HomerPipeline::new(repo.path());

    pipeline.run(&store, &config).await.unwrap();

    let t0 = Instant::now();
    let second = pipeline.run(&store, &config).await.unwrap();
    let elapsed = t0.elapsed();

    assert_eq!(
        second.extract_nodes, 0,
        "no-op update should skip extraction"
    );
    assert_eq!(
        second.extract_edges, 0,
        "no-op update should skip extraction"
    );
    assert!(
        elapsed <= threshold_ms("HOMER_PERF_INCREMENTAL_MS", 6000),
        "incremental no-op pipeline exceeded threshold: {elapsed:?}"
    );
}
