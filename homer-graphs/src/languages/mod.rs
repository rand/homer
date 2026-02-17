pub mod fallback;
pub mod go;
mod helpers;
pub mod java;
pub mod javascript;
pub mod python;
pub mod rust;
pub mod typescript;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::scope_graph::FileScopeGraph;
use crate::{ConventionQuery, FileGraph, HeuristicGraph, ResolutionTier, Result};

/// Trait implemented by each language's extraction support.
pub trait LanguageSupport: Send + Sync + std::fmt::Debug {
    /// Language identifier (e.g., "rust", "python").
    fn id(&self) -> &'static str;

    /// File extensions this language handles.
    fn extensions(&self) -> &'static [&'static str];

    /// Resolution tier this implementation provides.
    fn tier(&self) -> ResolutionTier;

    /// Tree-sitter language for parsing.
    fn tree_sitter_language(&self) -> tree_sitter::Language;

    /// Extract definitions, calls, and imports heuristically.
    fn extract_heuristic(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<HeuristicGraph>;

    /// Build a scope graph for a single file (Precise tier).
    ///
    /// Returns a `FileScopeGraph` with push/pop symbol nodes, scope nodes,
    /// and edges encoding the file's name binding structure for cross-file
    /// resolution via path-stitching.
    ///
    /// Default implementation returns `None` (not supported at Precise tier).
    /// Languages that support Precise tier should override this.
    fn build_scope_graph(
        &self,
        _tree: &tree_sitter::Tree,
        _source: &str,
        _path: &Path,
    ) -> Result<Option<FileScopeGraph>> {
        Ok(None)
    }

    /// Build a `FileGraph` combining definitions and references.
    ///
    /// Default implementation converts from the heuristic graph output.
    fn build_file_graph(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<FileGraph> {
        let heuristic = self.extract_heuristic(tree, source, path)?;
        Ok(FileGraph {
            file_path: heuristic.file_path,
            definitions: heuristic
                .definitions
                .into_iter()
                .map(|d| crate::Definition {
                    name: d.qualified_name,
                    kind: d.kind,
                    span: d.span,
                    doc_comment: d.doc_comment,
                })
                .collect(),
            references: heuristic
                .calls
                .into_iter()
                .map(|c| crate::Reference {
                    name: c.callee_name,
                    kind: crate::SymbolKind::Function,
                    span: c.span,
                })
                .collect(),
            scope_nodes: vec![],
            scope_edges: vec![],
        })
    }

    /// Tree-sitter queries for convention extraction.
    fn convention_queries(&self) -> &[ConventionQuery] {
        &[]
    }
}

/// Registry of all supported languages.
#[derive(Debug)]
pub struct LanguageRegistry {
    languages: HashMap<String, Arc<dyn LanguageSupport>>,
    extension_map: HashMap<String, String>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            languages: HashMap::new(),
            extension_map: HashMap::new(),
        };
        reg.register(Arc::new(rust::RustSupport));
        reg.register(Arc::new(python::PythonSupport));
        reg.register(Arc::new(typescript::TypeScriptSupport));
        reg.register(Arc::new(javascript::JavaScriptSupport));
        reg.register(Arc::new(go::GoSupport));
        reg.register(Arc::new(java::JavaSupport));
        reg.register(Arc::new(fallback::FallbackSupport));
        reg
    }

    fn register(&mut self, lang: Arc<dyn LanguageSupport>) {
        for ext in lang.extensions() {
            self.extension_map
                .insert((*ext).to_string(), lang.id().to_string());
        }
        self.languages.insert(lang.id().to_string(), lang);
    }

    /// Look up the language support for a file by its extension.
    pub fn for_file(&self, path: &Path) -> Option<Arc<dyn LanguageSupport>> {
        let ext = path.extension()?.to_str()?;
        let lang_id = self.extension_map.get(ext)?;
        self.languages.get(lang_id).cloned()
    }

    /// Get a language by its identifier.
    pub fn get(&self, id: &str) -> Option<Arc<dyn LanguageSupport>> {
        self.languages.get(id).cloned()
    }

    /// List all registered language IDs.
    pub fn language_ids(&self) -> Vec<&str> {
        self.languages.keys().map(String::as_str).collect()
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}
