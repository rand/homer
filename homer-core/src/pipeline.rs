use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::{info, instrument, warn};

use crate::analyze::behavioral::BehavioralAnalyzer;
use crate::analyze::centrality::CentralityAnalyzer;
use crate::analyze::community::CommunityAnalyzer;
use crate::analyze::convention::ConventionAnalyzer;
use crate::analyze::semantic::SemanticAnalyzer;
use crate::analyze::task_pattern::TaskPatternAnalyzer;
use crate::analyze::temporal::TemporalAnalyzer;
use crate::analyze::traits::Analyzer;
use crate::config::{AnalysisDepth, HomerConfig};
use crate::extract::document::DocumentExtractor;
use crate::extract::git::GitExtractor;
use crate::extract::github::GitHubExtractor;
use crate::extract::gitlab::GitLabExtractor;
use crate::extract::graph::GraphExtractor;
use crate::extract::prompt::PromptExtractor;
use crate::extract::structure::StructureExtractor;
use crate::extract::traits::Extractor;
use crate::llm::providers::create_provider;
use crate::progress::ProgressReporter;
use crate::render::agents_md::AgentsMdRenderer;
use crate::render::module_context::ModuleContextRenderer;
use crate::render::report::ReportRenderer;
use crate::render::risk_map::RiskMapRenderer;
use crate::render::skills::SkillsRenderer;
use crate::render::topos_spec::ToposSpecRenderer;
use crate::render::traits::Renderer;
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
    pub stage: String,
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
        let mut extractors: Vec<Box<dyn Extractor>> = vec![
            Box::new(GitExtractor::new(&self.repo_path)),
            Box::new(StructureExtractor::new(&self.repo_path)),
            Box::new(GraphExtractor::new(&self.repo_path)),
            Box::new(DocumentExtractor::new(&self.repo_path)),
        ];

        if let Some(gh) = GitHubExtractor::from_repo(&self.repo_path, config) {
            extractors.push(Box::new(gh));
        }
        if let Some(gl) = GitLabExtractor::from_repo(&self.repo_path, config) {
            extractors.push(Box::new(gl));
        }

        extractors.push(Box::new(PromptExtractor::new(&self.repo_path)));

        for ext in &extractors {
            let name = ext.name();
            let stage = format!("extract:{name}");

            // Check if extractor has new data to process.
            match ext.has_work(store).await {
                Ok(false) => {
                    info!(extractor = name, "Skipping (no new work)");
                    continue;
                }
                Ok(true) => {}
                Err(e) => {
                    // has_work() failure is non-fatal; proceed with extraction.
                    info!(extractor = name, error = %e, "has_work check failed, running anyway");
                }
            }

            match ext.extract(store, config).await {
                Ok(stats) => {
                    result.extract_nodes += stats.nodes_created;
                    result.extract_edges += stats.edges_created;
                    for (desc, err) in stats.errors {
                        result.errors.push(PipelineError {
                            stage: stage.clone(),
                            message: format!("{desc}: {err}"),
                        });
                    }
                }
                Err(e) => {
                    warn!(stage = %stage, error = %e, "Extraction failed");
                    result.errors.push(PipelineError {
                        stage,
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    #[instrument(skip_all)]
    async fn run_analysis(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        let analyzers = self.build_analyzer_list(config);
        let ordered = toposort_analyzers(&analyzers);

        for idx in ordered {
            let analyzer = &analyzers[idx];
            let name = analyzer.name();
            let stage = format!("analyze:{name}");

            // Check if analyzer needs to re-run.
            match analyzer.needs_rerun(store).await {
                Ok(false) => {
                    info!(analyzer = name, "Skipping (no rerun needed)");
                    continue;
                }
                Ok(true) => {}
                Err(e) => {
                    info!(analyzer = name, error = %e, "needs_rerun check failed, running anyway");
                }
            }

            match analyzer.analyze(store, config).await {
                Ok(stats) => {
                    result.analysis_results += stats.results_stored;
                    for (desc, err) in stats.errors {
                        result.errors.push(PipelineError {
                            stage: stage.clone(),
                            message: format!("{desc}: {err}"),
                        });
                    }
                }
                Err(e) => {
                    warn!(stage = %stage, error = %e, "Analysis failed");
                    result.errors.push(PipelineError {
                        stage,
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    /// Build the list of analyzers to run, conditionally including semantic.
    fn build_analyzer_list(&self, config: &HomerConfig) -> Vec<Box<dyn Analyzer>> {
        let mut analyzers: Vec<Box<dyn Analyzer>> = vec![
            Box::new(BehavioralAnalyzer),
            Box::new(CentralityAnalyzer::default()),
            Box::new(CommunityAnalyzer),
            Box::new(TemporalAnalyzer),
            Box::new(ConventionAnalyzer::new(&self.repo_path)),
            Box::new(TaskPatternAnalyzer),
        ];

        // Semantic analysis — LLM-powered, gated by config and depth.
        if config.llm.enabled && config.analysis.depth != AnalysisDepth::Shallow {
            match Self::create_llm_provider(config) {
                Ok(provider) => analyzers.push(Box::new(SemanticAnalyzer::new(provider))),
                Err(e) => {
                    info!(error = %e, "Skipping semantic analysis (LLM provider unavailable)");
                }
            }
        }

        analyzers
    }

    /// Create an LLM provider from config, reading the API key from the environment.
    fn create_llm_provider(
        config: &HomerConfig,
    ) -> crate::error::Result<Box<dyn crate::llm::LlmProvider>> {
        let api_key = std::env::var(&config.llm.api_key_env).map_err(|_| {
            crate::error::HomerError::Llm(crate::error::LlmError::Config(format!(
                "Environment variable {} not set",
                config.llm.api_key_env
            )))
        })?;

        create_provider(
            &config.llm.provider,
            &config.llm.model,
            &api_key,
            config.llm.base_url.as_deref(),
        )
    }

    /// All known renderer names and their stage labels.
    pub const ALL_RENDERER_NAMES: &[&str] = &[
        "agents-md",
        "module-ctx",
        "risk-map",
        "skills",
        "report",
        "topos-spec",
    ];

    /// Run only the named renderers against an existing store.
    ///
    /// Returns a `PipelineResult` with only the rendering fields populated.
    #[instrument(skip_all)]
    pub async fn run_renderers(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        renderer_names: &[&str],
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

        self.run_selected_renderers(store, config, renderer_names, &mut result)
            .await;

        result.duration = start.elapsed();
        Ok(result)
    }

    #[instrument(skip_all)]
    async fn run_rendering(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        result: &mut PipelineResult,
    ) {
        self.run_selected_renderers(store, config, Self::ALL_RENDERER_NAMES, result)
            .await;
    }

    async fn run_selected_renderers(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        names: &[&str],
        result: &mut PipelineResult,
    ) {
        let path = &self.repo_path;

        for name in names {
            let (stage, renderer): (String, Box<dyn Renderer>) = match *name {
                "agents-md" => ("render:agents_md".into(), Box::new(AgentsMdRenderer)),
                "module-ctx" => (
                    "render:module_context".into(),
                    Box::new(ModuleContextRenderer),
                ),
                "risk-map" => ("render:risk_map".into(), Box::new(RiskMapRenderer)),
                "skills" => ("render:skills".into(), Box::new(SkillsRenderer)),
                "report" => ("render:report".into(), Box::new(ReportRenderer)),
                "topos-spec" => ("render:topos_spec".into(), Box::new(ToposSpecRenderer)),
                unknown => {
                    result.errors.push(PipelineError {
                        stage: "render".into(),
                        message: format!("Unknown renderer: {unknown}"),
                    });
                    continue;
                }
            };

            match renderer.write(store, config, path).await {
                Ok(()) => result.artifacts_written += 1,
                Err(e) => {
                    warn!(stage = %stage, error = %e, "Renderer failed");
                    result.errors.push(PipelineError {
                        stage,
                        message: e.to_string(),
                    });
                }
            }
        }
    }
}

/// Topologically sort analyzers by their `produces()`/`requires()` declarations.
///
/// Uses Kahn's algorithm. If the dependency graph has cycles (should never happen),
/// remaining analyzers are appended in their original order.
fn toposort_analyzers(analyzers: &[Box<dyn Analyzer>]) -> Vec<usize> {
    let n = analyzers.len();

    // Build a map: AnalysisKind → index of the analyzer that produces it.
    let mut producer_of = std::collections::HashMap::new();
    for (i, a) in analyzers.iter().enumerate() {
        for kind in a.produces() {
            producer_of.insert(*kind, i);
        }
    }

    // Build adjacency: in_degree[i] = number of analyzers that must run before i.
    let mut in_degree = vec![0u32; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, a) in analyzers.iter().enumerate() {
        for kind in a.requires() {
            if let Some(&dep) = producer_of.get(kind) {
                if dep != i {
                    dependents[dep].push(i);
                    in_degree[i] += 1;
                }
            }
        }
    }

    // Deduplicate edges (an analyzer may require multiple kinds from the same producer).
    for deps in &mut dependents {
        deps.sort_unstable();
        deps.dedup();
    }
    // Recompute in-degree after dedup.
    in_degree.fill(0);
    for deps in &dependents {
        for &d in deps {
            in_degree[d] += 1;
        }
    }

    // Kahn's algorithm.
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &dep in &dependents[i] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                queue.push_back(dep);
            }
        }
    }

    // If there's a cycle, append remaining in original order as a fallback.
    if order.len() < n {
        warn!("Analyzer dependency graph has a cycle; appending remaining in original order");
        for i in 0..n {
            if !order.contains(&i) {
                order.push(i);
            }
        }
    }

    order
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

    #[test]
    fn toposort_respects_dependencies() {
        // Build the default analyzer list (no LLM → no semantic)
        let config = HomerConfig::default();
        let tmp = tempfile::tempdir().unwrap();
        let pipeline = HomerPipeline::new(tmp.path());
        let analyzers = pipeline.build_analyzer_list(&config);
        let order = toposort_analyzers(&analyzers);

        // Build name→position map
        let names: Vec<&str> = order.iter().map(|&i| analyzers[i].name()).collect();

        let pos = |name: &str| names.iter().position(|n| *n == name).unwrap();

        // behavioral must come before centrality (centrality requires ChangeFrequency)
        assert!(
            pos("behavioral") < pos("centrality"),
            "behavioral before centrality: {names:?}"
        );
        // centrality must come before community (community requires CompositeSalience)
        assert!(
            pos("centrality") < pos("community"),
            "centrality before community: {names:?}"
        );
        // community must come before temporal (temporal requires CommunityAssignment)
        assert!(
            pos("community") < pos("temporal"),
            "community before temporal: {names:?}"
        );
        // all analyzers are present
        assert_eq!(order.len(), analyzers.len());
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
