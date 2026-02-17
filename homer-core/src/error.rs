/// Top-level Homer error type.
#[derive(thiserror::Error, Debug)]
pub enum HomerError {
    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    #[error("Extraction error: {0}")]
    Extract(#[from] ExtractError),

    #[error("Analysis error: {0}")]
    Analyze(#[from] AnalyzeError),

    #[error("Render error: {0}")]
    Render(#[from] RenderError),

    #[error("Graph engine error: {0}")]
    Graph(#[from] homer_graphs::GraphError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
}

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Migration failed: {0}")]
    Migration(String),

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    #[error("Git error: {0}")]
    Git(String),

    #[error("Parse error in {path}: {message}")]
    Parse { path: String, message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum AnalyzeError {
    #[error("Insufficient data for analysis: {0}")]
    InsufficientData(String),

    #[error("Computation error: {0}")]
    Computation(String),
}

#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    #[error("Template error: {0}")]
    Template(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error("Invalid config: {0}")]
    Invalid(String),

    #[error("Parse error: {0}")]
    Parse(String),
}

#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("API error (HTTP {status}): {body}")]
    ApiError { status: u16, body: String },

    #[error("Response parse error: {0}")]
    Parse(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Cost budget exceeded: {0}")]
    BudgetExceeded(String),
}

pub type Result<T> = std::result::Result<T, HomerError>;
