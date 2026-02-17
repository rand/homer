pub mod call_graph;
pub mod diff;
pub mod import_graph;
pub mod languages;
pub mod scope_graph;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use languages::{LanguageRegistry, LanguageSupport};

/// Error type for the graph engine.
#[derive(thiserror::Error, Debug)]
pub enum GraphError {
    #[error("Parse error in {path}: {message}")]
    Parse { path: String, message: String },

    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("Tree-sitter error: {0}")]
    TreeSitter(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GraphError>;

// ── Resolution tiers ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResolutionTier {
    /// Full scope graph rules — precise cross-file resolution.
    Precise,
    /// Tree-sitter heuristic — within-file + import-based guesses.
    Heuristic,
    /// Package manifest only — module-level dependencies.
    Manifest,
    /// No analysis possible.
    Unsupported,
}

// ── Span type ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

impl From<tree_sitter::Range> for TextRange {
    fn from(r: tree_sitter::Range) -> Self {
        Self {
            start_byte: r.start_byte,
            end_byte: r.end_byte,
            start_row: r.start_point.row,
            start_col: r.start_point.column,
            end_row: r.end_point.row,
            end_col: r.end_point.column,
        }
    }
}

// ── Symbol kind ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Type,
    Variable,
    Module,
    Constant,
    Field,
}

// ── Heuristic extraction output ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicGraph {
    pub file_path: PathBuf,
    pub definitions: Vec<HeuristicDef>,
    pub calls: Vec<HeuristicCall>,
    pub imports: Vec<HeuristicImport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicDef {
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub span: TextRange,
    pub doc_comment: Option<DocCommentData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicCall {
    /// Qualified name of the containing function.
    pub caller: String,
    /// Name at call site (may be unqualified).
    pub callee_name: String,
    pub span: TextRange,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicImport {
    pub from_path: PathBuf,
    pub imported_name: String,
    pub target_path: Option<PathBuf>,
    pub confidence: f64,
}

// ── Doc comment data ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocCommentData {
    /// The doc comment text, stripped of syntax markers.
    pub text: String,
    /// Hash of the doc comment for freshness tracking.
    pub content_hash: u64,
    /// Documentation style detected.
    pub style: DocStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocStyle {
    Rustdoc,
    Jsdoc,
    Numpy,
    Google,
    Sphinx,
    Javadoc,
    Godoc,
    Other(String),
}

// ── Manifest dependency ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDependency {
    pub name: String,
    pub version: Option<String>,
    pub source_file: PathBuf,
    pub dev_only: bool,
}

// ── Convention query ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConventionQuery {
    pub name: String,
    pub description: String,
    pub query_source: String,
}

// ── File-level graph output (for precise tier) ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileGraph {
    pub file_path: PathBuf,
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,
    pub span: TextRange,
    pub doc_comment: Option<DocCommentData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub kind: SymbolKind,
    pub span: TextRange,
}
