use std::path::Path;

use crate::{HeuristicGraph, ResolutionTier, Result};

use super::LanguageSupport;

#[derive(Debug)]
pub struct JavaSupport;

impl LanguageSupport for JavaSupport {
    fn id(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
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
