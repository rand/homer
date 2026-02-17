// Import graph extraction from resolved scope graph references.
//
// Projects a file-level import graph: edges from importing files to
// imported files, with the imported symbols listed.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scope_graph::ResolvedReference;

/// A directed edge: file A imports symbol(s) from file B.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportEdge {
    pub source_file: PathBuf,
    pub target_file: PathBuf,
    pub symbols: Vec<String>,
    pub confidence: f64,
}

/// Projected import graph at the file level.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportGraph {
    pub edges: Vec<ImportEdge>,
}

/// Project a file-level import graph from resolved references.
///
/// Groups cross-file resolutions by (`source_file`, `target_file`) and collects
/// the imported symbol names.
pub fn project_import_graph(resolved: &[ResolvedReference]) -> ImportGraph {
    // Group by (ref_file, def_file) → symbols
    let mut grouped: HashMap<(PathBuf, PathBuf), (Vec<String>, f64)> = HashMap::new();

    for r in resolved {
        // Only cross-file references form import edges
        if r.reference_file == r.definition_file {
            continue;
        }

        let key = (r.reference_file.clone(), r.definition_file.clone());
        let entry = grouped.entry(key).or_insert_with(|| (Vec::new(), 0.0));
        if !entry.0.contains(&r.symbol) {
            entry.0.push(r.symbol.clone());
        }
        // Track minimum confidence across all symbols in this import
        if entry.1 == 0.0 || r.confidence < entry.1 {
            entry.1 = r.confidence;
        }
    }

    let edges = grouped
        .into_iter()
        .map(|((src, tgt), (symbols, confidence))| ImportEdge {
            source_file: src,
            target_file: tgt,
            symbols,
            confidence,
        })
        .collect();

    ImportGraph { edges }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SymbolKind;
    use crate::scope_graph::ScopeNodeId;

    #[test]
    fn groups_cross_file_refs() {
        let resolved = vec![
            ResolvedReference {
                reference_node: ScopeNodeId(1),
                definition_node: ScopeNodeId(10),
                symbol: "foo".to_string(),
                kind: Some(SymbolKind::Function),
                reference_file: PathBuf::from("main.rs"),
                definition_file: PathBuf::from("lib.rs"),
                confidence: 1.0,
            },
            ResolvedReference {
                reference_node: ScopeNodeId(2),
                definition_node: ScopeNodeId(11),
                symbol: "bar".to_string(),
                kind: Some(SymbolKind::Function),
                reference_file: PathBuf::from("main.rs"),
                definition_file: PathBuf::from("lib.rs"),
                confidence: 0.9,
            },
        ];

        let graph = project_import_graph(&resolved);
        assert_eq!(graph.edges.len(), 1, "Should group into one import edge");
        assert_eq!(graph.edges[0].symbols.len(), 2);
        assert!(graph.edges[0].symbols.contains(&"foo".to_string()));
        assert!(graph.edges[0].symbols.contains(&"bar".to_string()));
        // Minimum confidence
        assert!((graph.edges[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn excludes_same_file_refs() {
        let resolved = vec![ResolvedReference {
            reference_node: ScopeNodeId(1),
            definition_node: ScopeNodeId(2),
            symbol: "local".to_string(),
            kind: Some(SymbolKind::Function),
            reference_file: PathBuf::from("same.rs"),
            definition_file: PathBuf::from("same.rs"),
            confidence: 1.0,
        }];

        let graph = project_import_graph(&resolved);
        assert!(graph.edges.is_empty(), "Same-file refs should not produce import edges");
    }

    #[test]
    fn deduplicates_symbols() {
        let resolved = vec![
            ResolvedReference {
                reference_node: ScopeNodeId(1),
                definition_node: ScopeNodeId(10),
                symbol: "foo".to_string(),
                kind: Some(SymbolKind::Function),
                reference_file: PathBuf::from("a.rs"),
                definition_file: PathBuf::from("b.rs"),
                confidence: 1.0,
            },
            ResolvedReference {
                reference_node: ScopeNodeId(2),
                definition_node: ScopeNodeId(10),
                symbol: "foo".to_string(), // Same symbol, different reference site
                kind: Some(SymbolKind::Function),
                reference_file: PathBuf::from("a.rs"),
                definition_file: PathBuf::from("b.rs"),
                confidence: 1.0,
            },
        ];

        let graph = project_import_graph(&resolved);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].symbols.len(), 1, "Should deduplicate symbol names");
    }
}
