/// Top-level Homer error type.
///
/// All fallible operations in `homer-core` return [`Result<T, HomerError>`](Result).
/// Each variant wraps a domain-specific error enum, allowing callers to
/// match on the error source without losing type information.
#[derive(thiserror::Error, Debug)]
pub enum HomerError {
    /// Error from the hypergraph store layer (`SQLite` operations, migrations).
    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    /// Error during data extraction (git, structure, graph, document).
    #[error("Extraction error: {0}")]
    Extract(#[from] ExtractError),

    /// Error during analysis (behavioral, centrality, community, etc.).
    #[error("Analysis error: {0}")]
    Analyze(#[from] AnalyzeError),

    /// Error during output rendering (AGENTS.md, reports, etc.).
    #[error("Render error: {0}")]
    Render(#[from] RenderError),

    /// Error from the graph engine (tree-sitter parsing, scope graphs).
    #[error("Graph engine error: {0}")]
    Graph(#[from] homer_graphs::GraphError),

    /// Error in configuration parsing or validation.
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Error communicating with an LLM provider.
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
}

/// Errors from the SQLite-backed hypergraph store.
#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    /// Underlying `SQLite` operation failed.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Schema migration failed (version mismatch or DDL error).
    #[error("Migration failed: {0}")]
    Migration(String),

    /// A referenced node was not found in the store.
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    /// JSON serialization/deserialization of metadata failed.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Errors during the extraction pipeline phase.
#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    /// Git operation failed (clone, log, diff, etc.).
    #[error("Git error: {0}")]
    Git(String),

    /// Source file could not be parsed (tree-sitter failure).
    #[error("Parse error in {path}: {message}")]
    Parse {
        /// Path of the file that failed to parse.
        path: String,
        /// Description of the parse failure.
        message: String,
    },

    /// Filesystem I/O error during extraction.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors during the analysis pipeline phase.
#[derive(thiserror::Error, Debug)]
pub enum AnalyzeError {
    /// Not enough data in the store to run this analysis.
    #[error("Insufficient data for analysis: {0}")]
    InsufficientData(String),

    /// Algorithmic or numerical error during computation.
    #[error("Computation error: {0}")]
    Computation(String),
}

/// Errors during the rendering pipeline phase.
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    /// Output template processing failed.
    #[error("Template error: {0}")]
    Template(String),

    /// Filesystem I/O error writing rendered output.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors in Homer configuration parsing and validation.
#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    /// The configuration file does not exist at the expected path.
    #[error("Config file not found: {0}")]
    NotFound(String),

    /// Configuration values are present but semantically invalid.
    #[error("Invalid config: {0}")]
    Invalid(String),

    /// Configuration file syntax could not be parsed (TOML error).
    #[error("Parse error: {0}")]
    Parse(String),
}

/// Errors from LLM provider interactions (semantic analysis).
#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    /// Network-level failure connecting to the LLM provider.
    #[error("Network error: {0}")]
    Network(String),

    /// LLM API returned a non-success HTTP status.
    #[error("API error (HTTP {status}): {body}")]
    ApiError {
        /// HTTP status code from the provider.
        status: u16,
        /// Response body text.
        body: String,
    },

    /// LLM response could not be parsed into the expected format.
    #[error("Response parse error: {0}")]
    Parse(String),

    /// LLM configuration is missing or invalid (API key, model, etc.).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Cumulative LLM cost has exceeded the configured budget.
    #[error("Cost budget exceeded: {0}")]
    BudgetExceeded(String),
}

/// Convenience alias for `Result<T, HomerError>`.
pub type Result<T> = std::result::Result<T, HomerError>;
