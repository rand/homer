use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::{info, instrument, warn};

use crate::analyze::behavioral::BehavioralAnalyzer;
use crate::analyze::centrality::CentralityAnalyzer;
use crate::analyze::community::CommunityAnalyzer;
use crate::analyze::convention::ConventionAnalyzer;
use crate::analyze::temporal::TemporalAnalyzer;
use crate::analyze::task_pattern::TaskPatternAnalyzer;
use crate::analyze::traits::Analyzer;
use crate::config::HomerConfig;
use crate::extract::document::DocumentExtractor;
use crate::extract::git::GitExtractor;
use crate::extract::github::GitHubExtractor;
use crate::extract::gitlab::GitLabExtractor;
use crate::extract::graph::GraphExtractor;
use crate::extract::prompt::PromptExtractor;
use crate::extract::structure::StructureExtractor;
use crate::render::agents_md::AgentsMdRenderer;
use crate::render::module_context::ModuleContextRenderer;
use crate::render::risk_map::RiskMapRenderer;
use crate::render::report::ReportRenderer;
use crate::render::skills::SkillsRenderer;
use crate::render::topos_spec::ToposSpecRenderer;
use crate::render::traits::Renderer;
use crate::progress::ProgressReporter;
use crate::store::HomerStore;

/// Result of a full pipeline run.
#[derive(Debug)]
pub struct PipelineResult {
    pub extract_nodes: u64,
    pub extract_edges: u64,
    pub analysis_results: u64,
    pub artifacts_written: u32,
    pub errors: Vec<PipelineError>,
    pub duration: std::time::Duration,
}

/// A non-fatal error from one pipeline stage.
#[derive(Debug)]
pub struct PipelineError {
    pub stage: &'static str,
    pub message: String,
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.stage, self.message)
    }
}

/// Orchestrates the full Homer pipeline: extraction, analysis, and rendering.
#[derive(Debug)]
pub struct HomerPipeline {
    repo_path: PathBuf,
}

impl HomerPipeline {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }

    /// Run the full pipeline: Extract -> Analyze -> Render.
    pub async fn run(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<PipelineResult> {
        self.run_with_progress(store, config, &crate::progress::NoopReporter)
            .await
    }

    /// Run the full pipeline with a progress reporter for user-visible feedback.
    #[instrument(skip(self, store, config, progress), fields(repo = %self.repo_path.display()))]
    pub async fn run_with_progress(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        progress: &dyn ProgressReporter,
    ) -> crate::error::Result<PipelineResult> {
        let start = Instant::now();
        let mut result = PipelineResult {
            extract_nodes: 0,
            extract_edges: 0,
            analysis_results: 0,
            artifacts_written: 0,
            errors: Vec::new(),
            duration: std::time::Duration::ZERO,
        };

        info!(repo = %self.repo_path.display(), "Starting Homer pipeline");

        // ── Phase 1: Extraction ───────────────────────────────────
        progress.start("Extracting", None);
        self.run_extraction(store, config, &mut result).await;
        progress.message(&format!(
            "Extracted {} nodes, {} edges",
            result.extract_nodes, result.extract_edges
        ));
        progress.finish();

        // ── Phase 2: Analysis ─────────────────────────────────────
        progress.start("Analyzing", None);
        self.run_analysis(store, config, &mut result).await;
        progress.message(&format!("{} analyses computed", result.analysis_results));
        progress.finish();

        // ── Phase 3: Rendering ────────────────────────────────────
        progress.start("Rendering", None);
        self.run_rendering(store, config, &mut result).await;
        progress.message(&format!("{} artifacts written", result.artifacts_written));
        progress.finish();

        result.duration = start.elapsed();

        info!(
            nodes = result.extract_nodes,
            edges = result.extract_edges,
            analyses = result.analysis_results,
            artifacts = result.artifacts_written,
            errors = result.errors.len(),
            duration = ?result.duration,
            "Pipeline complete"
        );

        Ok(result)
    }

    #[instrument(skip_all)]
    async fn run_extraction(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        // 1. Git history (must come first — creates commits and file nodes)
        let git_ext = GitExtractor::new(&self.repo_path);
        match git_ext.extract(store, config).await {
            Ok(stats) => {
                result.extract_nodes += stats.nodes_created;
                result.extract_edges += stats.edges_created;
                for (path, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "extract:git",
                        message: format!("{path}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Git extraction failed (may not be a git repo)");
                result.errors.push(PipelineError {
                    stage: "extract:git",
                    message: e.to_string(),
                });
            }
        }

        // 2. Structure (file tree, manifests, CI config)
        let struct_ext = StructureExtractor::new(&self.repo_path);
        match struct_ext.extract(store, config).await {
            Ok(stats) => {
                result.extract_nodes += stats.nodes_created;
                result.extract_edges += stats.edges_created;
                for (path, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "extract:structure",
                        message: format!("{path}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Structure extraction failed");
                result.errors.push(PipelineError {
                    stage: "extract:structure",
                    message: e.to_string(),
                });
            }
        }

        // 3. Graph (call/import graphs via tree-sitter)
        let graph_ext = GraphExtractor::new(&self.repo_path);
        match graph_ext.extract(store, config).await {
            Ok(stats) => {
                result.extract_nodes += stats.nodes_created;
                result.extract_edges += stats.edges_created;
                for (path, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "extract:graph",
                        message: format!("{path}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Graph extraction failed");
                result.errors.push(PipelineError {
                    stage: "extract:graph",
                    message: e.to_string(),
                });
            }
        }

        // 4. Documents (Markdown cross-references)
        let doc_ext = DocumentExtractor::new(&self.repo_path);
        match doc_ext.extract(store, config).await {
            Ok(stats) => {
                result.extract_nodes += stats.nodes_created;
                result.extract_edges += stats.edges_created;
                for (path, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "extract:document",
                        message: format!("{path}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Document extraction failed");
                result.errors.push(PipelineError {
                    stage: "extract:document",
                    message: e.to_string(),
                });
            }
        }

        // 5. GitHub (PRs, issues, reviews — only if GitHub remote detected)
        self.run_github_extraction(store, config, result).await;

        // 5b. GitLab (MRs, issues, approvals — only if GitLab remote detected)
        self.run_gitlab_extraction(store, config, result).await;

        // 6. Prompts (agent rules always; sessions if opt-in)
        self.run_prompt_extraction(store, config, result).await;
    }

    async fn run_github_extraction(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        if let Some(gh_ext) = GitHubExtractor::from_repo(&self.repo_path, config) {
            match gh_ext.extract(store, config).await {
                Ok(stats) => {
                    result.extract_nodes += stats.nodes_created;
                    result.extract_edges += stats.edges_created;
                    for (desc, err) in stats.errors {
                        result.errors.push(PipelineError {
                            stage: "extract:github",
                            message: format!("{desc}: {err}"),
                        });
                    }
                }
                Err(e) => {
                    warn!(error = %e, "GitHub extraction failed");
                    result.errors.push(PipelineError {
                        stage: "extract:github",
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    async fn run_gitlab_extraction(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        if let Some(gl_ext) = GitLabExtractor::from_repo(&self.repo_path, config) {
            match gl_ext.extract(store, config).await {
                Ok(stats) => {
                    result.extract_nodes += stats.nodes_created;
                    result.extract_edges += stats.edges_created;
                    for (desc, err) in stats.errors {
                        result.errors.push(PipelineError {
                            stage: "extract:gitlab",
                            message: format!("{desc}: {err}"),
                        });
                    }
                }
                Err(e) => {
                    warn!(error = %e, "GitLab extraction failed");
                    result.errors.push(PipelineError {
                        stage: "extract:gitlab",
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    async fn run_prompt_extraction(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        let prompt_ext = PromptExtractor::new(&self.repo_path);
        match prompt_ext.extract(store, config).await {
            Ok(stats) => {
                result.extract_nodes += stats.nodes_created;
                result.extract_edges += stats.edges_created;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "extract:prompt",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Prompt extraction failed");
                result.errors.push(PipelineError {
                    stage: "extract:prompt",
                    message: e.to_string(),
                });
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    #[instrument(skip_all)]
    async fn run_analysis(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        // Behavioral analysis first (change frequency, bus factor, etc.)
        let behavioral = BehavioralAnalyzer;
        match behavioral.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:behavioral",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Behavioral analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:behavioral",
                    message: e.to_string(),
                });
            }
        }

        // Centrality analysis (PageRank, betweenness, HITS, composite salience)
        let centrality = CentralityAnalyzer::default();
        match centrality.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:centrality",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Centrality analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:centrality",
                    message: e.to_string(),
                });
            }
        }

        // Community detection + stability classification
        let community = CommunityAnalyzer;
        match community.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:community",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Community analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:community",
                    message: e.to_string(),
                });
            }
        }

        // Temporal analysis (centrality trends, drift, enhanced stability)
        let temporal = TemporalAnalyzer;
        match temporal.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:temporal",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Temporal analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:temporal",
                    message: e.to_string(),
                });
            }
        }

        // Convention analysis (naming, testing, error handling, doc style, agent rules)
        let convention = ConventionAnalyzer::new(&self.repo_path);
        match convention.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:convention",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Convention analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:convention",
                    message: e.to_string(),
                });
            }
        }

        // Task pattern analysis (prompt hotspots, correction hotspots, task patterns, vocabulary)
        let task_pattern = TaskPatternAnalyzer;
        match task_pattern.analyze(store, config).await {
            Ok(stats) => {
                result.analysis_results += stats.results_stored;
                for (desc, err) in stats.errors {
                    result.errors.push(PipelineError {
                        stage: "analyze:task_pattern",
                        message: format!("{desc}: {err}"),
                    });
                }
            }
            Err(e) => {
                warn!(error = %e, "Task pattern analysis failed");
                result.errors.push(PipelineError {
                    stage: "analyze:task_pattern",
                    message: e.to_string(),
                });
            }
        }
    }

    #[instrument(skip_all)]
    async fn run_rendering(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        let path = &self.repo_path;

        // Run all renderers concurrently — they are independent of each other
        let (agents, ctx, risk, skills, report, topos) = tokio::join!(
            AgentsMdRenderer.write(store, config, path),
            ModuleContextRenderer.write(store, config, path),
            RiskMapRenderer.write(store, config, path),
            SkillsRenderer.write(store, config, path),
            ReportRenderer.write(store, config, path),
            ToposSpecRenderer.write(store, config, path),
        );

        let outcomes: [(&str, crate::error::Result<()>); 6] = [
            ("render:agents_md", agents),
            ("render:module_context", ctx),
            ("render:risk_map", risk),
            ("render:skills", skills),
            ("render:report", report),
            ("render:topos_spec", topos),
        ];

        for (stage, outcome) in outcomes {
            match outcome {
                Ok(()) => result.artifacts_written += 1,
                Err(e) => {
                    warn!(stage, error = %e, "Renderer failed");
                    result.errors.push(PipelineError {
                        stage,
                        message: e.to_string(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    fn create_test_project(dir: &Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();

        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    greet();\n}\n\nfn greet() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
        )
        .unwrap();

        std::fs::write(
            dir.join("README.md"),
            "# Test\n\n## Overview\n\nA test project using [main](src/main.rs).\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn pipeline_runs_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let pipeline = HomerPipeline::new(tmp.path());

        let result = pipeline.run(&store, &config).await.unwrap();

        // Git extraction will fail (not a git repo), but pipeline should continue
        assert!(
            result.extract_nodes > 0,
            "Should create nodes from structure/graph extraction"
        );
        assert!(result.extract_edges > 0, "Should create edges");

        // AGENTS.md should have been written
        let agents_path = tmp.path().join("AGENTS.md");
        assert!(agents_path.exists(), "AGENTS.md should be created");

        let content = std::fs::read_to_string(&agents_path).unwrap();
        assert!(content.contains("# AGENTS.md"), "Should have title");
        assert!(
            content.contains("cargo build"),
            "Should detect Cargo project"
        );

        // Pipeline should not abort on git failure
        let git_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.stage == "extract:git")
            .collect();
        assert!(
            !git_errors.is_empty(),
            "Should have git error (not a git repo)"
        );

        // Non-git extractors should succeed
        assert!(
            result.artifacts_written >= 1,
            "Should write at least AGENTS.md"
        );
    }

    #[tokio::test]
    async fn pipeline_handles_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let pipeline = HomerPipeline::new(tmp.path());

        let result = pipeline.run(&store, &config).await.unwrap();

        // Should complete without panic
        assert!(result.duration.as_nanos() > 0);
    }
}
