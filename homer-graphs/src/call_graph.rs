// Call graph projection from resolved scope graph references.
//
// Given a set of resolved references (from path-stitching), project a call graph
// where each edge represents a function calling another function.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scope_graph::{ResolvedReference, ScopeGraph, ScopeNodeId, ScopeNodeKind};
use crate::{SymbolKind, TextRange};

/// A directed edge in the call graph: caller → callee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEdge {
    pub caller_name: String,
    pub caller_file: PathBuf,
    pub callee_name: String,
    pub callee_file: PathBuf,
    pub call_span: Option<TextRange>,
    pub confidence: f64,
}

/// Projected call graph from resolved scope graph references.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallGraph {
    pub edges: Vec<CallEdge>,
}

/// Project a call graph from resolved references and the scope graph.
///
/// For each resolved reference where the definition is a function:
/// 1. Find the enclosing function of the reference (the "caller").
/// 2. The resolved definition is the "callee".
/// 3. Add caller → callee edge.
pub fn project_call_graph<S: BuildHasher>(
    sg: &ScopeGraph,
    resolved: &[ResolvedReference],
    enclosing_functions: &HashMap<ScopeNodeId, ScopeNodeId, S>,
) -> CallGraph {
    let mut edges = Vec::new();

    for resolution in resolved {
        // Only project calls to functions/methods
        if !matches!(
            resolution.kind,
            Some(SymbolKind::Function | SymbolKind::Type)
        ) {
            continue;
        }

        // Find the enclosing function of the reference site
        let Some(&caller_id) = enclosing_functions.get(&resolution.reference_node) else {
            continue; // Reference not inside a function — skip (e.g., top-level)
        };

        let Some(src_node) = sg.get_node(caller_id) else {
            continue;
        };

        let ScopeNodeKind::PopSymbol { symbol: ref name } = src_node.kind else {
            continue;
        };

        let Some(dst_node) = sg.get_node(resolution.definition_node) else {
            continue;
        };

        let ref_node = sg.get_node(resolution.reference_node);
        let call_span = ref_node.and_then(|n| n.span);

        edges.push(CallEdge {
            caller_name: name.clone(),
            caller_file: src_node.file_path.clone(),
            callee_name: resolution.symbol.clone(),
            callee_file: dst_node.file_path.clone(),
            call_span,
            confidence: resolution.confidence,
        });
    }

    CallGraph { edges }
}

/// Compute a map from each `PushSymbol` (reference) to its enclosing function
/// (`PopSymbol` with `SymbolKind::Function`) using span containment.
///
/// Returns the map in terms of the file-level `ScopeNodeId`s. Caller should
/// remap using the `id_map` returned by `ScopeGraph::add_file_graph`.
pub fn compute_enclosing_functions(
    file_graph: &crate::scope_graph::FileScopeGraph,
) -> HashMap<ScopeNodeId, ScopeNodeId> {
    use crate::scope_graph::ScopeNodeKind;

    // Collect all function definitions with their spans
    let functions: Vec<_> = file_graph
        .nodes
        .iter()
        .filter(|n| {
            n.symbol_kind == Some(SymbolKind::Function)
                && matches!(n.kind, ScopeNodeKind::PopSymbol { .. })
                && n.span.is_some()
        })
        .collect();

    let mut enclosing = HashMap::new();

    for node in &file_graph.nodes {
        let ScopeNodeKind::PushSymbol { .. } = &node.kind else {
            continue;
        };
        let Some(ref_span) = node.span else {
            continue;
        };

        // Find the smallest enclosing function by span containment
        let mut best: Option<(ScopeNodeId, usize)> = None;
        for func in &functions {
            let func_span = func.span.unwrap(); // filtered above
            if span_contains(func_span, ref_span) {
                let size = span_size(func_span);
                if best.is_none_or(|(_, s)| size < s) {
                    best = Some((func.id, size));
                }
            }
        }

        if let Some((func_id, _)) = best {
            enclosing.insert(node.id, func_id);
        }
    }

    enclosing
}

fn span_contains(outer: TextRange, inner: TextRange) -> bool {
    (outer.start_row < inner.start_row
        || (outer.start_row == inner.start_row && outer.start_col <= inner.start_col))
        && (outer.end_row > inner.end_row
            || (outer.end_row == inner.end_row && outer.end_col >= inner.end_col))
}

fn span_size(span: TextRange) -> usize {
    (span.end_row - span.start_row + 1) * 1000 + (span.end_col - span.start_col)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope_graph::*;

    #[test]
    fn projects_simple_call() {
        // Build a scope graph: main() calls foo()
        // main is defined at PopSymbol("main"), references foo via PushSymbol("foo")
        let nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("test.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "main".to_string(),
                },
                file_path: PathBuf::from("test.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::PushSymbol {
                    symbol: "foo".to_string(),
                },
                file_path: PathBuf::from("test.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
            ScopeNode {
                id: ScopeNodeId(3),
                kind: ScopeNodeKind::Scope,
                file_path: PathBuf::from("test.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(4),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "foo".to_string(),
                },
                file_path: PathBuf::from("test.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
        ];

        let edges = vec![
            ScopeEdge {
                id: ScopeEdgeId(0),
                source: ScopeNodeId(2),
                target: ScopeNodeId(3),
                precedence: 0,
            },
            ScopeEdge {
                id: ScopeEdgeId(1),
                source: ScopeNodeId(3),
                target: ScopeNodeId(4),
                precedence: 0,
            },
        ];

        let file_graph = FileScopeGraph {
            file_path: PathBuf::from("test.rs"),
            nodes,
            edges,
            root_scope: ScopeNodeId(0),
            export_nodes: vec![],
            import_nodes: vec![],
        };

        let mut graph = ScopeGraph::new();
        let id_map = graph.add_file_graph(&file_graph);

        let resolved = graph.resolve_all();
        assert_eq!(resolved.len(), 1);

        // The push node (ref to "foo") is enclosed in "main"
        let push_new_id = id_map[&ScopeNodeId(2)];
        let main_new_id = id_map[&ScopeNodeId(1)];
        let enclosing = HashMap::from([(push_new_id, main_new_id)]);

        let cg = project_call_graph(&graph, &resolved, &enclosing);
        assert_eq!(cg.edges.len(), 1);
        assert_eq!(cg.edges[0].caller_name, "main");
        assert_eq!(cg.edges[0].callee_name, "foo");
    }

    #[test]
    fn skips_non_function_references() {
        let resolved = vec![ResolvedReference {
            reference_node: ScopeNodeId(1),
            definition_node: ScopeNodeId(2),
            symbol: "MY_CONST".to_string(),
            kind: Some(SymbolKind::Constant), // Not a function
            reference_file: PathBuf::from("a.rs"),
            definition_file: PathBuf::from("b.rs"),
            confidence: 1.0,
        }];

        let graph = ScopeGraph::new();
        let enclosing: HashMap<ScopeNodeId, ScopeNodeId> = HashMap::new();

        let cg = project_call_graph(&graph, &resolved, &enclosing);
        assert!(cg.edges.is_empty());
    }
}
