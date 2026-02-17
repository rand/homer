use homer_core::store::HomerStore;
use homer_core::types::{AnalysisKind, NodeFilter, NodeKind};
use homer_test::{TestRepo, run_pipeline_with_store};

// ── Minimal Rust Fixture ─────────────────────────────────────────

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn minimal_rust_full_pipeline() {
    let repo = TestRepo::minimal_rust();
    let (result, store) = run_pipeline_with_store(repo.path()).await;

    // Pipeline should complete with nodes and edges
    assert!(result.extract_nodes > 0, "Should extract nodes");
    assert!(result.extract_edges > 0, "Should extract edges");

    // Git extraction should succeed (we have a real git repo)
    let git_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.stage == "extract:git")
        .collect();
    assert!(
        git_errors.is_empty(),
        "Git extraction should succeed: {git_errors:?}"
    );

    // Should have commit nodes from git history
    let commit_filter = NodeFilter {
        kind: Some(NodeKind::Commit),
        ..Default::default()
    };
    let commits = store.find_nodes(&commit_filter).await.unwrap();
    assert!(
        commits.len() >= 5,
        "Should have at least 5 commits, got {}",
        commits.len()
    );

    // Should have contributor nodes
    let contrib_filter = NodeFilter {
        kind: Some(NodeKind::Contributor),
        ..Default::default()
    };
    let contributors = store.find_nodes(&contrib_filter).await.unwrap();
    assert!(
        !contributors.is_empty(),
        "Should have at least 1 contributor"
    );

    // Should have file nodes from structure extraction
    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await.unwrap();
    assert!(
        files.len() >= 3,
        "Should have at least 3 source files, got {}",
        files.len()
    );

    // Should have module nodes
    let mod_filter = NodeFilter {
        kind: Some(NodeKind::Module),
        ..Default::default()
    };
    let modules = store.find_nodes(&mod_filter).await.unwrap();
    assert!(
        modules.len() >= 2,
        "Should have root + src modules, got {}",
        modules.len()
    );

    // Should have external dependency nodes from Cargo.toml
    let dep_filter = NodeFilter {
        kind: Some(NodeKind::ExternalDep),
        ..Default::default()
    };
    let deps = store.find_nodes(&dep_filter).await.unwrap();
    assert!(
        deps.len() >= 2,
        "Should have serde + tokio deps, got {}",
        deps.len()
    );

    // Should have function nodes from graph extraction
    let fn_filter = NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let functions = store.find_nodes(&fn_filter).await.unwrap();
    assert!(!functions.is_empty(), "Should have function nodes");

    // Should have behavioral analysis results
    let freq_results = store
        .get_analyses_by_kind(AnalysisKind::ChangeFrequency)
        .await
        .unwrap();
    assert!(
        !freq_results.is_empty(),
        "Should have change frequency results"
    );

    // Should have centrality analysis results (PageRank on call graph)
    let pr_results = store
        .get_analyses_by_kind(AnalysisKind::PageRank)
        .await
        .unwrap();
    assert!(
        !pr_results.is_empty(),
        "Should have PageRank results from call graph"
    );

    // Should have composite salience
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await
        .unwrap();
    assert!(
        !salience_results.is_empty(),
        "Should have composite salience results"
    );

    // AGENTS.md should be generated
    assert!(
        result.artifacts_written >= 1,
        "Should write at least AGENTS.md"
    );
    let agents_path = repo.path().join("AGENTS.md");
    assert!(agents_path.exists(), "AGENTS.md should be created");

    let content = std::fs::read_to_string(&agents_path).unwrap();
    assert!(content.contains("# AGENTS.md"), "Should have title");
    assert!(
        content.contains("## Build & Test Commands"),
        "Should have build section"
    );
    assert!(
        content.contains("cargo build"),
        "Should detect Cargo project"
    );
    assert!(content.contains("## Module Map"), "Should have module map");
    assert!(
        content.contains("## Change Patterns"),
        "Should have change patterns"
    );
    assert!(
        content.contains("## Conventions"),
        "Should have conventions"
    );
    assert!(content.contains("rust"), "Should detect Rust language");
}

#[tokio::test]
async fn minimal_rust_has_bus_factor_analysis() {
    let repo = TestRepo::minimal_rust();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    let bus_results = store
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await
        .unwrap();
    assert!(!bus_results.is_empty(), "Should have bus factor analysis");

    // Single contributor = bus factor 1
    for result in &bus_results {
        let bf = result
            .data
            .get("bus_factor")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        assert_eq!(bf, 1, "Bus factor should be 1 for single-contributor repo");
    }
}

#[tokio::test]
async fn minimal_rust_has_centrality_analysis() {
    let repo = TestRepo::minimal_rust();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    // PageRank should be computed on the call graph
    let pr_results = store
        .get_analyses_by_kind(AnalysisKind::PageRank)
        .await
        .unwrap();
    assert!(!pr_results.is_empty(), "Should have PageRank results");

    // Each PageRank result should have score and rank
    for r in &pr_results {
        assert!(
            r.data.get("pagerank").is_some(),
            "PageRank result should have score"
        );
        assert!(r.data.get("rank").is_some(), "Should have rank");
    }

    // HITS should be computed
    let hits_results = store
        .get_analyses_by_kind(AnalysisKind::HITSScore)
        .await
        .unwrap();
    assert!(!hits_results.is_empty(), "Should have HITS results");

    for r in &hits_results {
        assert!(r.data.get("hub_score").is_some(), "Should have hub_score");
        assert!(
            r.data.get("authority_score").is_some(),
            "Should have authority_score"
        );
        let classification = r
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(
            ["Hub", "Authority", "Both", "Neither"].contains(&classification),
            "Invalid HITS classification: {classification}"
        );
    }

    // Composite salience should combine centrality + behavioral
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await
        .unwrap();
    assert!(
        !salience_results.is_empty(),
        "Should have composite salience results"
    );

    for r in &salience_results {
        let val = r
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap();
        assert!(val >= 0.0, "Salience score should be non-negative");
        let classification = r
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(
            [
                "HotCritical",
                "CriticalSilo",
                "FoundationalStable",
                "ActiveLocalized",
                "Background"
            ]
            .contains(&classification),
            "Invalid salience classification: {classification}"
        );
    }
}

// ── Multi-Language Fixture ───────────────────────────────────────

#[tokio::test]
async fn multi_lang_detects_all_languages() {
    let repo = TestRepo::multi_lang();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await.unwrap();

    let languages: Vec<_> = files
        .iter()
        .filter_map(|f| f.metadata.get("language").and_then(|v| v.as_str()))
        .collect();

    assert!(languages.contains(&"rust"), "Should detect Rust files");
    assert!(languages.contains(&"python"), "Should detect Python files");
    assert!(
        languages.contains(&"typescript"),
        "Should detect TypeScript files"
    );
}

#[tokio::test]
async fn multi_lang_extracts_functions_across_languages() {
    let repo = TestRepo::multi_lang();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    let fn_filter = NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let functions = store.find_nodes(&fn_filter).await.unwrap();
    let fn_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();

    // Rust functions
    assert!(
        fn_names.iter().any(|n| n.contains("main")),
        "Should find Rust main, got: {fn_names:?}"
    );

    // Python functions
    assert!(
        fn_names.iter().any(|n| n.contains("fetch_data")),
        "Should find Python fetch_data, got: {fn_names:?}"
    );

    // TypeScript functions
    assert!(
        fn_names.iter().any(|n| n.contains("greet")),
        "Should find TypeScript greet, got: {fn_names:?}"
    );
}

// ── Documented Fixture ───────────────────────────────────────────

#[tokio::test]
async fn documented_project_extracts_documents() {
    let repo = TestRepo::documented();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    let doc_filter = NodeFilter {
        kind: Some(NodeKind::Document),
        ..Default::default()
    };
    let docs = store.find_nodes(&doc_filter).await.unwrap();

    assert!(!docs.is_empty(), "Should extract document nodes");

    let doc_names: Vec<&str> = docs.iter().map(|d| d.name.as_str()).collect();
    assert!(
        doc_names.iter().any(|n| n.contains("README")),
        "Should find README, got: {doc_names:?}"
    );
}

#[tokio::test]
async fn documented_project_has_doc_comments_on_functions() {
    let repo = TestRepo::documented();
    let (_result, store) = run_pipeline_with_store(repo.path()).await;

    let fn_filter = NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let functions = store.find_nodes(&fn_filter).await.unwrap();

    // At least one function should have doc_comment metadata
    let with_docs: Vec<_> = functions
        .iter()
        .filter(|f| f.metadata.contains_key("doc_comment"))
        .collect();

    assert!(
        !with_docs.is_empty(),
        "Should have functions with doc comments. Functions: {:?}",
        functions
            .iter()
            .map(|f| (&f.name, f.metadata.keys().collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    );
}

// ── Error Handling ───────────────────────────────────────────────

#[tokio::test]
async fn pipeline_survives_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (result, _store) = run_pipeline_with_store(dir.path()).await;

    // Should not panic, should complete
    assert!(
        result.duration.as_nanos() > 0,
        "Should have non-zero duration"
    );
}

#[tokio::test]
async fn pipeline_survives_malformed_source_file() {
    let repo = TestRepo::minimal_rust();

    // Add a binary file with .rs extension (will fail tree-sitter parsing)
    std::fs::write(repo.path().join("src/broken.rs"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();

    let (result, _store) = run_pipeline_with_store(repo.path()).await;

    // Pipeline should complete without aborting
    assert!(result.extract_nodes > 0, "Should still extract other nodes");
    assert!(
        result.artifacts_written >= 1,
        "Should still produce AGENTS.md"
    );
}

// ── Module Context + Risk Map ────────────────────────────────────

#[tokio::test]
async fn pipeline_generates_risk_map() {
    let repo = TestRepo::minimal_rust();
    let (_result, _store) = run_pipeline_with_store(repo.path()).await;

    let risk_path = repo.path().join("homer-risk.json");
    assert!(risk_path.exists(), "homer-risk.json should be created");

    let content = std::fs::read_to_string(&risk_path).unwrap();
    let risk_map: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(risk_map["version"], "1.0");
    assert!(
        risk_map["risk_areas"].is_array(),
        "Should have risk_areas array"
    );
    assert!(
        risk_map["safe_areas"].is_array(),
        "Should have safe_areas array"
    );
}

#[tokio::test]
async fn pipeline_generates_module_context() {
    let repo = TestRepo::minimal_rust();
    let (_result, _store) = run_pipeline_with_store(repo.path()).await;

    let src_ctx = repo.path().join("src/.context.md");
    assert!(
        src_ctx.exists(),
        "src/.context.md should be created"
    );

    let content = std::fs::read_to_string(&src_ctx).unwrap();
    assert!(content.contains("# src"), "Should have module title");
    assert!(
        content.contains("## Key Entities"),
        "Should have entities section"
    );
    assert!(
        content.contains("## Change Profile"),
        "Should have change profile"
    );
}

#[tokio::test]
async fn agents_md_has_load_bearing_code() {
    let repo = TestRepo::minimal_rust();
    let (_result, _store) = run_pipeline_with_store(repo.path()).await;

    let agents_path = repo.path().join("AGENTS.md");
    let content = std::fs::read_to_string(&agents_path).unwrap();
    assert!(
        content.contains("## Load-Bearing Code"),
        "AGENTS.md should have Load-Bearing Code section"
    );
}

// ── AGENTS.md Merge Behavior ─────────────────────────────────────

#[tokio::test]
async fn agents_md_preserves_human_sections() {
    let repo = TestRepo::minimal_rust();

    // Run pipeline once
    let store = homer_core::store::sqlite::SqliteStore::in_memory().unwrap();
    let config = homer_core::config::HomerConfig::default();
    let pipeline = homer_core::pipeline::HomerPipeline::new(repo.path());
    pipeline.run(&store, &config).await.unwrap();

    // Add human content with preserve markers
    let agents_path = repo.path().join("AGENTS.md");
    let original = std::fs::read_to_string(&agents_path).unwrap();
    let modified = original.replace(
        "## Conventions",
        "## Conventions\n<!-- homer:preserve -->\nHuman-written convention: always use Result types.\n<!-- /homer:preserve -->"
    );
    std::fs::write(&agents_path, &modified).unwrap();

    // Run pipeline again (re-render)
    let store2 = homer_core::store::sqlite::SqliteStore::in_memory().unwrap();
    let pipeline2 = homer_core::pipeline::HomerPipeline::new(repo.path());
    pipeline2.run(&store2, &config).await.unwrap();

    // Human content should be preserved
    let final_content = std::fs::read_to_string(&agents_path).unwrap();
    assert!(
        final_content.contains("Human-written convention"),
        "Should preserve human content through re-render"
    );
}
