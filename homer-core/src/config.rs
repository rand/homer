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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomerSection {
    pub version: String,
}

impl Default for HomerSection {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSection {
    pub depth: AnalysisDepth,
    pub llm_salience_threshold: f64,
    pub max_llm_batch_size: u32,
}

impl Default for AnalysisSection {
    fn default() -> Self {
        Self {
            depth: AnalysisDepth::Standard,
            llm_salience_threshold: 0.7,
            max_llm_batch_size: 50,
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
    pub gitlab: GitLabExtractionConfig,
}

impl Default for ExtractionSection {
    fn default() -> Self {
        Self {
            max_commits: 2000,
            structure: StructureExtractionConfig::default(),
            documents: DocumentExtractionConfig::default(),
            prompts: PromptExtractionConfig::default(),
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
pub struct GraphSection {
    pub languages: LanguageConfig,
}

impl Default for GraphSection {
    fn default() -> Self {
        Self {
            languages: LanguageConfig::Auto,
        }
    }
}

/// Language selection: `"auto"` to detect from file extensions, or an explicit list.
#[derive(Debug, Clone)]
pub enum LanguageConfig {
    Auto,
    Explicit(Vec<String>),
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self::Auto
    }
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
}

impl Default for RenderersSection {
    fn default() -> Self {
        Self {
            enabled: vec!["agents-md".into(), "module-ctx".into(), "risk-map".into()],
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
