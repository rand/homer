// Graph diffing — compute added/removed definitions and edges between snapshots.
//
// When a file is re-analyzed, compare old and new subgraphs to produce a diff
// that feeds the temporal analyzer.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{HeuristicDef, HeuristicGraph, SymbolKind, TextRange};

/// Diff between two versions of a file's graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphDiff {
    pub added_definitions: Vec<DiffDef>,
    pub removed_definitions: Vec<DiffDef>,
    pub added_edges: Vec<(String, String)>,
    pub removed_edges: Vec<(String, String)>,
    pub renamed_symbols: Vec<(String, String)>,
}

/// A definition entry in a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffDef {
    pub name: String,
    pub kind: SymbolKind,
    pub span: TextRange,
}

impl From<&HeuristicDef> for DiffDef {
    fn from(d: &HeuristicDef) -> Self {
        Self {
            name: d.qualified_name.clone(),
            kind: d.kind,
            span: d.span,
        }
    }
}

/// Compute the diff between an old and new heuristic graph for the same file.
///
/// Definitions are matched by qualified name (exact match). Definitions with
/// the same span but different names are detected as renames.
pub fn diff_heuristic_graphs(old: &HeuristicGraph, new: &HeuristicGraph) -> GraphDiff {
    let old_defs: HashMap<&str, &HeuristicDef> = old
        .definitions
        .iter()
        .map(|d| (d.qualified_name.as_str(), d))
        .collect();

    let new_defs: HashMap<&str, &HeuristicDef> = new
        .definitions
        .iter()
        .map(|d| (d.qualified_name.as_str(), d))
        .collect();

    let old_names: HashSet<&str> = old_defs.keys().copied().collect();
    let new_names: HashSet<&str> = new_defs.keys().copied().collect();

    // Added definitions: in new but not old
    let added_definitions: Vec<DiffDef> = new_names
        .difference(&old_names)
        .filter_map(|name| new_defs.get(name))
        .map(|d| DiffDef::from(*d))
        .collect();

    // Removed definitions: in old but not new (unless renamed)
    let mut removed_definitions: Vec<DiffDef> = Vec::new();
    let mut renamed_symbols: Vec<(String, String)> = Vec::new();

    for &name in old_names.difference(&new_names) {
        let old_def = old_defs[name];
        // Check if this was renamed: look for a new def at the same span
        let rename_target = new.definitions.iter().find(|d| {
            spans_overlap(&d.span, &old_def.span) && !old_names.contains(d.qualified_name.as_str())
        });

        if let Some(target) = rename_target {
            renamed_symbols.push((name.to_string(), target.qualified_name.clone()));
        } else {
            removed_definitions.push(DiffDef::from(old_def));
        }
    }

    // Call edge diffs
    let old_edges: HashSet<(&str, &str)> = old
        .calls
        .iter()
        .map(|c| (c.caller.as_str(), c.callee_name.as_str()))
        .collect();

    let new_edges: HashSet<(&str, &str)> = new
        .calls
        .iter()
        .map(|c| (c.caller.as_str(), c.callee_name.as_str()))
        .collect();

    let added_edges = new_edges
        .difference(&old_edges)
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect();

    let removed_edges = old_edges
        .difference(&new_edges)
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect();

    GraphDiff {
        added_definitions,
        removed_definitions,
        added_edges,
        removed_edges,
        renamed_symbols,
    }
}

/// Check if two spans overlap (same region of source code, suggesting a rename).
fn spans_overlap(a: &TextRange, b: &TextRange) -> bool {
    // Same start row is a strong signal; within 2 rows is a weaker signal.
    a.start_row == b.start_row || (a.start_byte < b.end_byte && b.start_byte < a.end_byte)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{HeuristicCall, HeuristicDef, HeuristicGraph, SymbolKind, TextRange};

    fn make_def(name: &str, row: usize) -> HeuristicDef {
        HeuristicDef {
            name: name.split("::").last().unwrap_or(name).to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            span: TextRange {
                start_byte: row * 100,
                end_byte: row * 100 + 50,
                start_row: row,
                start_col: 0,
                end_row: row + 5,
                end_col: 0,
            },
            doc_comment: None,
        }
    }

    #[allow(clippy::similar_names)]
    fn make_call(caller: &str, callee: &str) -> HeuristicCall {
        HeuristicCall {
            caller: caller.to_string(),
            callee_name: callee.to_string(),
            span: TextRange {
                start_byte: 0,
                end_byte: 10,
                start_row: 0,
                start_col: 0,
                end_row: 0,
                end_col: 10,
            },
            confidence: 0.8,
        }
    }

    #[test]
    fn detects_added_and_removed_definitions() {
        let old = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("foo", 0), make_def("bar", 10)],
            calls: vec![],
            imports: vec![],
        };
        let new = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("foo", 0), make_def("baz", 20)],
            calls: vec![],
            imports: vec![],
        };

        let diff = diff_heuristic_graphs(&old, &new);
        assert_eq!(diff.added_definitions.len(), 1);
        assert_eq!(diff.added_definitions[0].name, "baz");
        assert_eq!(diff.removed_definitions.len(), 1);
        assert_eq!(diff.removed_definitions[0].name, "bar");
    }

    #[test]
    fn detects_renames_by_span() {
        let old = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("old_name", 5)],
            calls: vec![],
            imports: vec![],
        };
        let new = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("new_name", 5)], // Same row
            calls: vec![],
            imports: vec![],
        };

        let diff = diff_heuristic_graphs(&old, &new);
        assert!(
            diff.removed_definitions.is_empty(),
            "Renamed should not appear as removed"
        );
        assert_eq!(diff.renamed_symbols.len(), 1);
        assert_eq!(diff.renamed_symbols[0].0, "old_name");
        assert_eq!(diff.renamed_symbols[0].1, "new_name");
    }

    #[test]
    fn detects_added_and_removed_edges() {
        let old = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("main", 0), make_def("foo", 10)],
            calls: vec![make_call("main", "foo")],
            imports: vec![],
        };
        let new = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("main", 0), make_def("foo", 10)],
            calls: vec![make_call("main", "foo"), make_call("main", "bar")],
            imports: vec![],
        };

        let diff = diff_heuristic_graphs(&old, &new);
        assert_eq!(diff.added_edges.len(), 1);
        assert_eq!(diff.added_edges[0], ("main".to_string(), "bar".to_string()));
        assert!(diff.removed_edges.is_empty());
    }

    #[test]
    fn identical_graphs_produce_empty_diff() {
        let graph = HeuristicGraph {
            file_path: PathBuf::from("test.rs"),
            definitions: vec![make_def("foo", 0)],
            calls: vec![make_call("foo", "bar")],
            imports: vec![],
        };

        let diff = diff_heuristic_graphs(&graph, &graph);
        assert!(diff.added_definitions.is_empty());
        assert!(diff.removed_definitions.is_empty());
        assert!(diff.added_edges.is_empty());
        assert!(diff.removed_edges.is_empty());
        assert!(diff.renamed_symbols.is_empty());
    }
}
