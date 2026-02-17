use std::path::Path;

use crate::{HeuristicGraph, ResolutionTier, Result};

use super::LanguageSupport;

#[derive(Debug)]
pub struct RustSupport;

impl LanguageSupport for RustSupport {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn tier(&self) -> ResolutionTier {
        // Phase 1: heuristic only. Phase 2+: upgrade to Precise.
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn extract_heuristic(
        &self,
        _tree: &tree_sitter::Tree,
        _source: &str,
        path: &Path,
    ) -> Result<HeuristicGraph> {
        // TODO: P1.09 — implement Rust heuristic extraction
        Ok(HeuristicGraph {
            file_path: path.to_path_buf(),
            definitions: Vec::new(),
            calls: Vec::new(),
            imports: Vec::new(),
        })
    }
}
