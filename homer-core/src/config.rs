use serde::{Deserialize, Serialize};

/// Analysis depth level — gates which features are enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisDepth {
    Shallow,
    #[default]
    Standard,
    Deep,
    Full,
}

/// Top-level Homer configuration, matching `.homer/config.toml`.
///
/// Call [`HomerConfig::with_depth_overrides`] after loading to apply
/// the depth table (CLI.md) limits to extraction and analysis settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HomerConfig {
    #[serde(default)]
    pub homer: HomerSection,
    #[serde(default)]
    pub analysis: AnalysisSection,
    #[serde(default)]
    pub extraction: ExtractionSection,
    #[serde(default)]
    pub graph: GraphSection,
    #[serde(default)]
    pub renderers: RenderersSection,
    #[serde(default)]
    pub llm: LlmSection,
    #[serde(default)]
    pub mcp: McpSection,
}

impl HomerConfig {
    /// Apply the depth table from CLI.md to extraction and analysis settings.
    ///
    /// | Level    | max_commits | GitHub PRs | GitHub Issues | LLM batch |
    /// |----------|-------------|------------|---------------|-----------|
    /// | shallow  | 500         | 0 (skip)   | 0 (skip)      | 0         |
    /// | standard | 2000        | 200        | 500           | 50        |
    /// | deep     | 0 (all)     | 500        | 1000          | 200       |
    /// | full     | 0 (all)     | 0 (all)    | 0 (all)       | max       |
    #[must_use]
    pub fn with_depth_overrides(mut self) -> Self {
        match self.analysis.depth {
            AnalysisDepth::Shallow => {
                self.extraction.max_commits = 500;
                self.extraction.github.max_pr_history = 0;
                self.extraction.github.max_issue_history = 0;
                self.extraction.gitlab.max_mr_history = 0;
                self.extraction.gitlab.max_issue_history = 0;
                self.analysis.max_llm_batch_size = 0;
            }
            AnalysisDepth::Standard => {
                self.extraction.max_commits = 2000;
                self.extraction.github.max_pr_history = 200;
                self.extraction.github.max_issue_history = 500;
                self.extraction.gitlab.max_mr_history = 200;
                self.extraction.gitlab.max_issue_history = 500;
                self.analysis.max_llm_batch_size = 50;
            }
            AnalysisDepth::Deep => {
                self.extraction.max_commits = 0; // 0 = unlimited
                self.extraction.github.max_pr_history = 500;
                self.extraction.github.max_issue_history = 1000;
                self.extraction.gitlab.max_mr_history = 500;
                self.extraction.gitlab.max_issue_history = 1000;
                self.analysis.max_llm_batch_size = 200;
            }
            AnalysisDepth::Full => {
                self.extraction.max_commits = 0;
                self.extraction.github.max_pr_history = 0; // 0 = unlimited
                self.extraction.github.max_issue_history = 0;
                self.extraction.gitlab.max_mr_history = 0;
                self.extraction.gitlab.max_issue_history = 0;
                // LLM batch: keep config value (depth_adjusted_batch_size handles usize::MAX)
            }
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HomerSection {
    pub version: String,
    /// Custom database path (overrides default `.homer/homer.db`).
    pub db_path: Option<String>,
}

impl Default for HomerSection {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
            db_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSection {
    pub depth: AnalysisDepth,
    pub llm_salience_threshold: f64,
    pub max_llm_batch_size: u32,
    #[serde(default)]
    pub invalidation: InvalidationPolicy,
}

impl Default for AnalysisSection {
    fn default() -> Self {
        Self {
            depth: AnalysisDepth::Standard,
            llm_salience_threshold: 0.7,
            max_llm_batch_size: 50,
            invalidation: InvalidationPolicy::default(),
        }
    }
}

/// Controls how analysis results are invalidated when the graph changes.
///
/// The default is coarse-grained: any topology change invalidates all centrality
/// scores, and semantic summaries are only invalidated on direct content changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidationPolicy {
    /// If true, any graph topology change invalidates all centrality scores
    /// (`PageRank`, `BetweennessCentrality`, `HITSScore`, `CompositeSalience`)
    /// for every node.
    pub global_centrality_on_topology_change: bool,
    /// If true, only invalidate semantic summaries (`SemanticSummary`,
    /// `DesignRationale`, `InvariantDescription`) when a node's own
    /// `content_hash` changes — not when its neighbors change.
    pub conservative_semantic_invalidation: bool,
}

impl Default for InvalidationPolicy {
    fn default() -> Self {
        Self {
            global_centrality_on_topology_change: true,
            conservative_semantic_invalidation: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionSection {
    pub max_commits: u32,
    #[serde(default)]
    pub structure: StructureExtractionConfig,
    #[serde(default)]
    pub documents: DocumentExtractionConfig,
    #[serde(default)]
    pub prompts: PromptExtractionConfig,
    #[serde(default)]
    pub github: GitHubExtractionConfig,
    #[serde(default)]
    pub gitlab: GitLabExtractionConfig,
}

impl Default for ExtractionSection {
    fn default() -> Self {
        Self {
            max_commits: 2000,
            structure: StructureExtractionConfig::default(),
            documents: DocumentExtractionConfig::default(),
            prompts: PromptExtractionConfig::default(),
            github: GitHubExtractionConfig::default(),
            gitlab: GitLabExtractionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureExtractionConfig {
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

impl Default for StructureExtractionConfig {
    fn default() -> Self {
        Self {
            include_patterns: vec![
                "**/*.rs".into(),
                "**/*.py".into(),
                "**/*.ts".into(),
                "**/*.tsx".into(),
                "**/*.js".into(),
                "**/*.jsx".into(),
                "**/*.go".into(),
                "**/*.java".into(),
            ],
            exclude_patterns: vec![
                "**/node_modules/**".into(),
                "**/vendor/**".into(),
                "**/target/**".into(),
                "**/.git/**".into(),
                "**/dist/**".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionConfig {
    pub enabled: bool,
    pub include_doc_comments: bool,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

impl Default for DocumentExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_doc_comments: true,
            include_patterns: vec![
                "README*".into(),
                "CONTRIBUTING*".into(),
                "ARCHITECTURE*".into(),
                "CHANGELOG*".into(),
                "docs/**/*.md".into(),
                "doc/**/*.md".into(),
                "adr/**/*.md".into(),
            ],
            exclude_patterns: vec!["**/node_modules/**".into(), "**/vendor/**".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct PromptExtractionConfig {
    pub enabled: bool,
    pub sources: Vec<String>,
    pub redact_sensitive: bool,
    pub store_full_text: bool,
    pub hash_session_ids: bool,
}

impl Default for PromptExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sources: vec!["claude-code".into(), "agent-rules".into()],
            redact_sensitive: true,
            store_full_text: false,
            hash_session_ids: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubExtractionConfig {
    /// Environment variable holding the GitHub personal access token.
    pub token_env: String,
    /// Max PRs to fetch on initial run.
    pub max_pr_history: u32,
    /// Max issues to fetch on initial run.
    pub max_issue_history: u32,
    /// Whether to fetch PR/issue comments as metadata.
    pub include_comments: bool,
    /// Whether to fetch PR reviews and create Reviewed edges.
    pub include_reviews: bool,
}

impl Default for GitHubExtractionConfig {
    fn default() -> Self {
        Self {
            token_env: "GITHUB_TOKEN".to_string(),
            max_pr_history: 500,
            max_issue_history: 1000,
            include_comments: true,
            include_reviews: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabExtractionConfig {
    /// Environment variable holding the GitLab personal access token.
    pub token_env: String,
    /// Max merge requests to fetch on initial run.
    pub max_mr_history: u32,
    /// Max issues to fetch on initial run.
    pub max_issue_history: u32,
    /// Whether to include MR comments as metadata.
    pub include_comments: bool,
    /// Whether to include approvals/reviews.
    pub include_reviews: bool,
}

impl Default for GitLabExtractionConfig {
    fn default() -> Self {
        Self {
            token_env: "GITLAB_TOKEN".to_string(),
            max_mr_history: 500,
            max_issue_history: 1000,
            include_comments: true,
            include_reviews: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphSection {
    pub languages: LanguageConfig,
    #[serde(default)]
    pub snapshots: SnapshotsConfig,
}

impl Default for GraphSection {
    fn default() -> Self {
        Self {
            languages: LanguageConfig::Auto,
            snapshots: SnapshotsConfig::default(),
        }
    }
}

/// Controls automatic graph snapshot creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotsConfig {
    /// Create snapshots at tagged releases.
    pub at_releases: bool,
    /// Create snapshots every N commits (0 = disabled).
    pub every_n_commits: u32,
}

impl Default for SnapshotsConfig {
    fn default() -> Self {
        Self {
            at_releases: true,
            every_n_commits: 100,
        }
    }
}

/// Language selection: `"auto"` to detect from file extensions, or an explicit list.
#[derive(Debug, Clone, Default)]
pub enum LanguageConfig {
    #[default]
    Auto,
    Explicit(Vec<String>),
}

impl Serialize for LanguageConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Explicit(langs) => langs.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for LanguageConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = LanguageConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(r#""auto" or a list of language names"#)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == "auto" {
                    Ok(LanguageConfig::Auto)
                } else {
                    Err(E::custom(format!("expected \"auto\", got \"{v}\"")))
                }
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut langs = Vec::new();
                while let Some(val) = seq.next_element()? {
                    langs.push(val);
                }
                Ok(LanguageConfig::Explicit(langs))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderersSection {
    pub enabled: Vec<String>,
    #[serde(default, rename = "agents-md")]
    pub agents_md: AgentsMdConfig,
    #[serde(default, rename = "module-ctx")]
    pub module_ctx: ModuleContextConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default, rename = "topos-spec")]
    pub topos_spec: ToposSpecConfig,
    #[serde(default)]
    pub report: ReportConfig,
    #[serde(default, rename = "risk-map")]
    pub risk_map: RiskMapConfig,
}

impl Default for RenderersSection {
    fn default() -> Self {
        Self {
            enabled: vec!["agents-md".into(), "module-ctx".into(), "risk-map".into()],
            agents_md: AgentsMdConfig::default(),
            module_ctx: ModuleContextConfig::default(),
            skills: SkillsConfig::default(),
            topos_spec: ToposSpecConfig::default(),
            report: ReportConfig::default(),
            risk_map: RiskMapConfig::default(),
        }
    }
}

/// Per-renderer config for `agents-md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsMdConfig {
    /// Output file path relative to repo root.
    pub output_path: String,
    /// Max entries in the Load-Bearing Code table.
    pub max_load_bearing: u32,
    /// Max entries in the Change Patterns tables.
    pub max_change_patterns: u32,
    /// Max entries in the Key Design Decisions list.
    pub max_design_decisions: u32,
    /// How to handle existing AGENTS.md: `auto`, `diff`, `merge`, `overwrite`.
    pub circularity_mode: String,
}

impl Default for AgentsMdConfig {
    fn default() -> Self {
        Self {
            output_path: "AGENTS.md".to_string(),
            max_load_bearing: 20,
            max_change_patterns: 10,
            max_design_decisions: 10,
            circularity_mode: "auto".to_string(),
        }
    }
}

/// Per-renderer config for `module-ctx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleContextConfig {
    /// Filename for per-directory context files.
    pub filename: String,
    /// Whether to generate one file per directory.
    pub per_directory: bool,
}

impl Default for ModuleContextConfig {
    fn default() -> Self {
        Self {
            filename: ".context.md".to_string(),
            per_directory: true,
        }
    }
}

/// Per-renderer config for `skills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Output directory for skill files (relative to repo root).
    pub output_dir: String,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            output_dir: ".claude/skills/".to_string(),
        }
    }
}

/// Per-renderer config for `topos-spec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToposSpecConfig {
    /// Output directory for spec files (relative to repo root).
    pub output_dir: String,
    /// Spec format: `topos`.
    pub format: String,
}

impl Default for ToposSpecConfig {
    fn default() -> Self {
        Self {
            output_dir: "spec/".to_string(),
            format: "topos".to_string(),
        }
    }
}

/// Per-renderer config for `report`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportConfig {
    /// Output file path relative to repo root.
    pub output_path: String,
    /// Report format: `html` or `markdown`.
    pub format: String,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            output_path: "homer-report.html".to_string(),
            format: "html".to_string(),
        }
    }
}

/// Per-renderer config for `risk-map`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RiskMapConfig {
    /// Output file path relative to repo root.
    pub output_path: String,
}

impl Default for RiskMapConfig {
    fn default() -> Self {
        Self {
            output_path: "homer-risk.json".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSection {
    pub provider: String,
    pub model: String,
    /// Environment variable holding the API key.
    pub api_key_env: String,
    /// Base URL override (for custom providers).
    pub base_url: Option<String>,
    /// Max concurrent LLM requests.
    pub max_concurrent: u32,
    /// USD cost budget; 0 = unlimited.
    pub cost_budget: f64,
    /// Whether LLM features are enabled.
    pub enabled: bool,
}

impl Default for LlmSection {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: None,
            max_concurrent: 5,
            cost_budget: 0.0,
            enabled: false, // opt-in by default
        }
    }
}

/// MCP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSection {
    /// Transport type: "stdio" or "sse".
    pub transport: String,
    /// Host for SSE transport.
    pub host: String,
    /// Port for SSE transport.
    pub port: u16,
}

impl Default for McpSection {
    fn default() -> Self {
        Self {
            transport: "stdio".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_config_defaults() {
        let config = RenderersSection::default();
        assert_eq!(config.agents_md.output_path, "AGENTS.md");
        assert_eq!(config.agents_md.max_load_bearing, 20);
        assert_eq!(config.agents_md.max_change_patterns, 10);
        assert_eq!(config.agents_md.max_design_decisions, 10);
        assert_eq!(config.agents_md.circularity_mode, "auto");
        assert_eq!(config.module_ctx.filename, ".context.md");
        assert!(config.module_ctx.per_directory);
        assert_eq!(config.skills.output_dir, ".claude/skills/");
        assert_eq!(config.topos_spec.output_dir, "spec/");
        assert_eq!(config.topos_spec.format, "topos");
        assert_eq!(config.report.output_path, "homer-report.html");
        assert_eq!(config.report.format, "html");
        assert_eq!(config.risk_map.output_path, "homer-risk.json");
    }

    #[test]
    fn renderer_config_from_toml() {
        let toml_str = r#"
[renderers]
enabled = ["agents-md", "risk-map"]

[renderers.agents-md]
max_load_bearing = 30
max_change_patterns = 5
max_design_decisions = 15
circularity_mode = "overwrite"

[renderers.module-ctx]
filename = ".module.md"
per_directory = false

[renderers.skills]
output_dir = "skills/"

[renderers.topos-spec]
output_dir = "docs/spec/"
format = "topos"

[renderers.report]
format = "markdown"
"#;
        let config: HomerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.renderers.enabled,
            vec!["agents-md".to_string(), "risk-map".to_string()]
        );
        assert_eq!(config.renderers.agents_md.max_load_bearing, 30);
        assert_eq!(config.renderers.agents_md.max_change_patterns, 5);
        assert_eq!(config.renderers.agents_md.max_design_decisions, 15);
        assert_eq!(config.renderers.agents_md.circularity_mode, "overwrite");
        assert_eq!(config.renderers.module_ctx.filename, ".module.md");
        assert!(!config.renderers.module_ctx.per_directory);
        assert_eq!(config.renderers.skills.output_dir, "skills/");
        assert_eq!(config.renderers.topos_spec.output_dir, "docs/spec/");
        assert_eq!(config.renderers.report.format, "markdown");
    }

    #[test]
    fn renderer_config_partial_toml_uses_defaults() {
        let toml_str = r#"
[renderers]
enabled = ["agents-md"]

[renderers.agents-md]
max_load_bearing = 50
"#;
        let config: HomerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.renderers.agents_md.max_load_bearing, 50);
        // Unspecified fields get defaults
        assert_eq!(config.renderers.agents_md.output_path, "AGENTS.md");
        assert_eq!(config.renderers.agents_md.max_change_patterns, 10);
        assert_eq!(config.renderers.agents_md.circularity_mode, "auto");
        assert_eq!(config.renderers.module_ctx.filename, ".context.md");
        assert!(config.renderers.module_ctx.per_directory);
    }

    #[test]
    fn homer_section_defaults() {
        let config = HomerConfig::default();
        assert_eq!(config.homer.version, "0.1.0");
        assert!(config.homer.db_path.is_none());
    }

    #[test]
    fn homer_section_with_db_path() {
        let toml_str = r#"
[homer]
version = "0.2.0"
db_path = "/custom/path/homer.db"
"#;
        let config: HomerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.homer.version, "0.2.0");
        assert_eq!(
            config.homer.db_path.as_deref(),
            Some("/custom/path/homer.db")
        );
    }

    #[test]
    fn graph_snapshots_defaults() {
        let config = HomerConfig::default();
        assert!(config.graph.snapshots.at_releases);
        assert_eq!(config.graph.snapshots.every_n_commits, 100);
    }

    #[test]
    fn graph_snapshots_from_toml() {
        let toml_str = r#"
[graph]
languages = "auto"

[graph.snapshots]
at_releases = false
every_n_commits = 50
"#;
        let config: HomerConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.graph.snapshots.at_releases);
        assert_eq!(config.graph.snapshots.every_n_commits, 50);
    }

    #[test]
    fn mcp_section_defaults() {
        let config = HomerConfig::default();
        assert_eq!(config.mcp.transport, "stdio");
        assert_eq!(config.mcp.host, "127.0.0.1");
        assert_eq!(config.mcp.port, 3000);
    }

    #[test]
    fn mcp_section_from_toml() {
        let toml_str = r#"
[mcp]
transport = "sse"
host = "0.0.0.0"
port = 8080
"#;
        let config: HomerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mcp.transport, "sse");
        assert_eq!(config.mcp.host, "0.0.0.0");
        assert_eq!(config.mcp.port, 8080);
    }

    #[test]
    fn depth_overrides_shallow() {
        let mut config = HomerConfig::default();
        config.analysis.depth = AnalysisDepth::Shallow;
        let config = config.with_depth_overrides();
        assert_eq!(config.extraction.max_commits, 500);
        assert_eq!(config.extraction.github.max_pr_history, 0);
        assert_eq!(config.extraction.gitlab.max_mr_history, 0);
        assert_eq!(config.analysis.max_llm_batch_size, 0);
    }

    #[test]
    fn depth_overrides_standard() {
        let config = HomerConfig::default().with_depth_overrides();
        assert_eq!(config.extraction.max_commits, 2000);
        assert_eq!(config.extraction.github.max_pr_history, 200);
        assert_eq!(config.extraction.github.max_issue_history, 500);
        assert_eq!(config.analysis.max_llm_batch_size, 50);
    }

    #[test]
    fn depth_overrides_deep() {
        let mut config = HomerConfig::default();
        config.analysis.depth = AnalysisDepth::Deep;
        let config = config.with_depth_overrides();
        assert_eq!(config.extraction.max_commits, 0); // unlimited
        assert_eq!(config.extraction.github.max_pr_history, 500);
        assert_eq!(config.analysis.max_llm_batch_size, 200);
    }

    #[test]
    fn depth_overrides_full() {
        let mut config = HomerConfig::default();
        config.analysis.depth = AnalysisDepth::Full;
        let config = config.with_depth_overrides();
        assert_eq!(config.extraction.max_commits, 0);
        assert_eq!(config.extraction.github.max_pr_history, 0); // unlimited
        assert_eq!(config.extraction.github.max_issue_history, 0);
    }
}
