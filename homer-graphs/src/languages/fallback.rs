use std::path::Path;

use crate::{HeuristicGraph, ResolutionTier, Result};

use super::LanguageSupport;

/// Generic tree-sitter heuristic extractor for languages without dedicated support.
/// Attempts basic function/class definition extraction using common AST patterns.
#[derive(Debug)]
pub struct FallbackSupport;

impl LanguageSupport for FallbackSupport {
    fn id(&self) -> &'static str {
        "fallback"
    }

    fn extensions(&self) -> &'static [&'static str] {
        // Fallback doesn't claim any extensions — it's used when no other language matches
        // but a tree-sitter grammar is available.
        &[]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        // Fallback doesn't have its own grammar; callers must provide one.
        // This method shouldn't be called directly on the fallback.
        panic!("FallbackSupport::tree_sitter_language() should not be called directly")
    }

    fn extract_heuristic(
        &self,
        _tree: &tree_sitter::Tree,
        _source: &str,
        path: &Path,
    ) -> Result<HeuristicGraph> {
        Ok(HeuristicGraph {
            file_path: path.to_path_buf(),
            definitions: Vec::new(),
            calls: Vec::new(),
            imports: Vec::new(),
        })
    }
}
