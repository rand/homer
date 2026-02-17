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
}

impl Default for ExtractionSection {
    fn default() -> Self {
        Self {
            max_commits: 2000,
            structure: StructureExtractionConfig::default(),
            documents: DocumentExtractionConfig::default(),
            prompts: PromptExtractionConfig::default(),
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
            exclude_patterns: vec![
                "**/node_modules/**".into(),
                "**/vendor/**".into(),
            ],
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LanguageConfig {
    Auto,
    Explicit(Vec<String>),
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderersSection {
    pub enabled: Vec<String>,
}

impl Default for RenderersSection {
    fn default() -> Self {
        Self {
            enabled: vec![
                "agents-md".into(),
                "module-ctx".into(),
                "risk-map".into(),
            ],
        }
    }
}
