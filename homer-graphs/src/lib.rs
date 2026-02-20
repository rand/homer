//! Tree-sitter based graph extraction for 6 languages.
//!
//! Produces [`FileGraph`] (precise tier) and [`HeuristicGraph`] (heuristic tier)
//! representations of source files, including definitions, references, calls,
//! imports, and scope graphs.

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
    /// Source file could not be parsed by tree-sitter.
    #[error("Parse error in {path}: {message}")]
    Parse {
        /// Path of the file that failed to parse.
        path: String,
        /// Description of the parse failure.
        message: String,
    },

    /// The file's language is not supported by the graph engine.
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    /// Internal tree-sitter error (query compilation, node access, etc.).
    #[error("Tree-sitter error: {0}")]
    TreeSitter(String),

    /// Filesystem I/O error reading source files.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for `Result<T, GraphError>`.
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

/// Byte and line/column span within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    /// Byte offset of the span start.
    pub start_byte: usize,
    /// Byte offset of the span end (exclusive).
    pub end_byte: usize,
    /// Zero-based starting row.
    pub start_row: usize,
    /// Zero-based starting column.
    pub start_col: usize,
    /// Zero-based ending row.
    pub end_row: usize,
    /// Zero-based ending column.
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

/// Classification of a source-code symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Function or method definition.
    Function,
    /// Type definition (struct, class, enum, interface, trait).
    Type,
    /// Variable or parameter binding.
    Variable,
    /// Module or namespace.
    Module,
    /// Named constant.
    Constant,
    /// Struct/class field.
    Field,
}

// ── Heuristic extraction output ────────────────────────────────────

/// Heuristic-tier graph extracted from a single source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicGraph {
    /// Path to the source file.
    pub file_path: PathBuf,
    /// Symbol definitions found in the file.
    pub definitions: Vec<HeuristicDef>,
    /// Intra-file call relationships.
    pub calls: Vec<HeuristicCall>,
    /// Import/use statements.
    pub imports: Vec<HeuristicImport>,
}

/// A symbol definition discovered by heuristic extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicDef {
    /// Simple (unqualified) name.
    pub name: String,
    /// Fully qualified name (e.g. `module::Class::method`).
    pub qualified_name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Source location of the definition.
    pub span: TextRange,
    /// Extracted doc comment, if present.
    pub doc_comment: Option<DocCommentData>,
}

/// A call relationship discovered by heuristic extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicCall {
    /// Qualified name of the containing function.
    pub caller: String,
    /// Name at call site (may be unqualified).
    pub callee_name: String,
    /// Source location of the call expression.
    pub span: TextRange,
    /// Confidence score (0.0–1.0) for this call edge.
    pub confidence: f64,
}

/// An import/use statement discovered by heuristic extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeuristicImport {
    /// File containing the import statement.
    pub from_path: PathBuf,
    /// Name being imported.
    pub imported_name: String,
    /// Resolved target file path, if known.
    pub target_path: Option<PathBuf>,
    /// Confidence score (0.0–1.0) for the resolution.
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

/// Documentation style detected from comment syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocStyle {
    /// Rust `///` or `//!` doc comments.
    Rustdoc,
    /// JavaScript/TypeScript `/** ... */` with `@param`/`@returns`.
    Jsdoc,
    /// Python NumPy-style docstrings.
    Numpy,
    /// Python Google-style docstrings.
    Google,
    /// Python Sphinx ``:param:`` style docstrings.
    Sphinx,
    /// Java `/** ... */` with `@param`/`@return`.
    Javadoc,
    /// Go `//` comment blocks preceding declarations.
    Godoc,
    /// Unrecognized or custom documentation style.
    Other(String),
}

// ── Manifest dependency ────────────────────────────────────────────

/// A dependency declared in a package manifest (e.g. `Cargo.toml`, `package.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDependency {
    /// Package/crate name.
    pub name: String,
    /// Version constraint string, if specified.
    pub version: Option<String>,
    /// Path to the manifest file declaring this dependency.
    pub source_file: PathBuf,
    /// Whether this is a dev/test-only dependency.
    pub dev_only: bool,
}

// ── Convention query ───────────────────────────────────────────────

/// A tree-sitter query for detecting coding conventions.
#[derive(Debug, Clone)]
pub struct ConventionQuery {
    /// Short name for the convention (e.g. `"snake_case_functions"`).
    pub name: String,
    /// Human-readable description of what this checks.
    pub description: String,
    /// Tree-sitter query source (S-expression).
    pub query_source: String,
}

// ── File-level graph output (for precise tier) ─────────────────────

/// Precise-tier graph extracted from a single source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileGraph {
    /// Path to the source file.
    pub file_path: PathBuf,
    /// Symbol definitions found in the file.
    pub definitions: Vec<Definition>,
    /// Symbol references found in the file.
    pub references: Vec<Reference>,
    /// Scope nodes for precise resolution (populated by Precise tier languages).
    pub scope_nodes: Vec<scope_graph::ScopeNode>,
    /// Scope edges connecting scope nodes.
    pub scope_edges: Vec<scope_graph::ScopeEdge>,
}

/// A symbol definition in the precise-tier graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Definition {
    /// Symbol name.
    pub name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Source location of the definition.
    pub span: TextRange,
    /// Extracted doc comment, if present.
    pub doc_comment: Option<DocCommentData>,
}

/// A symbol reference (usage) in the precise-tier graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    /// Referenced symbol name.
    pub name: String,
    /// Expected kind of the referenced symbol.
    pub kind: SymbolKind,
    /// Source location of the reference.
    pub span: TextRange,
}
