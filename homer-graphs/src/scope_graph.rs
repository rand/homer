// Scope graph data structures and path-stitching resolution.
//
// A scope graph encodes a program's name binding structure:
// - Push symbol nodes represent references (uses of a name)
// - Pop symbol nodes represent definitions (declarations of a name)
// - Scope nodes define visibility boundaries (blocks, modules, namespaces)
// - Edges connect scopes, creating paths from references to definitions
//
// Name resolution = finding a valid path from a push node to a pop node
// where the pushed/popped symbols match.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{SymbolKind, TextRange};

// ── Node IDs ──────────────────────────────────────────────────────────

/// Opaque ID for a scope graph node. Unique within a single `ScopeGraph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeNodeId(pub u32);

/// Opaque ID for a scope graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeEdgeId(pub u32);

// ── Node kinds ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScopeNodeKind {
    /// Root scope of a file.
    Root,
    /// An interior scope (block, function body, class body, module).
    Scope,
    /// A reference: pushes a symbol onto the matching stack.
    PushSymbol { symbol: String },
    /// A definition: pops a symbol from the matching stack.
    PopSymbol { symbol: String },
    /// Export boundary — connects file-internal definitions to cross-file resolution.
    ExportScope,
    /// Import boundary — connects cross-file resolution to file-internal references.
    ImportScope,
}

// ── Nodes and edges ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNode {
    pub id: ScopeNodeId,
    pub kind: ScopeNodeKind,
    /// The file this node belongs to.
    pub file_path: PathBuf,
    /// Optional source span (definitions and references have spans).
    pub span: Option<TextRange>,
    /// Symbol kind (for definitions/references with known kinds).
    pub symbol_kind: Option<SymbolKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeEdge {
    pub id: ScopeEdgeId,
    pub source: ScopeNodeId,
    pub target: ScopeNodeId,
    /// Edge precedence (lower = preferred during resolution).
    pub precedence: u8,
}

// ── File subgraph ─────────────────────────────────────────────────────

/// A file's contribution to the scope graph. Each file produces an isolated
/// subgraph that connects to others through export/import scope nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileScopeGraph {
    pub file_path: PathBuf,
    pub nodes: Vec<ScopeNode>,
    pub edges: Vec<ScopeEdge>,
    /// Which node is this file's root scope.
    pub root_scope: ScopeNodeId,
    /// Export nodes that make definitions available to other files.
    pub export_nodes: Vec<ScopeNodeId>,
    /// Import nodes that consume definitions from other files.
    pub import_nodes: Vec<ScopeNodeId>,
}

// ── Resolved reference ────────────────────────────────────────────────

/// A reference that has been resolved to a definition through path-stitching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedReference {
    /// The push-symbol node (reference site).
    pub reference_node: ScopeNodeId,
    /// The pop-symbol node (definition site).
    pub definition_node: ScopeNodeId,
    /// The symbol name that was resolved.
    pub symbol: String,
    /// Symbol kind at the definition site.
    pub kind: Option<SymbolKind>,
    /// File containing the reference.
    pub reference_file: PathBuf,
    /// File containing the definition.
    pub definition_file: PathBuf,
    /// Confidence in this resolution (1.0 = exact match, lower = ambiguous).
    pub confidence: f64,
}

// ── Partial path (for path-stitching) ─────────────────────────────────

/// A partial path during resolution. Tracks the symbol stack and visited nodes.
#[derive(Debug, Clone)]
struct PartialPath {
    /// Stack of symbols pushed but not yet popped.
    symbol_stack: Vec<String>,
    /// Current node in the traversal.
    current_node: ScopeNodeId,
    /// Nodes visited so far (cycle detection).
    visited: HashSet<ScopeNodeId>,
    /// Starting reference node.
    start_node: ScopeNodeId,
}

// ── Scope Graph ───────────────────────────────────────────────────────

/// The combined scope graph for an entire project (or analysis scope).
///
/// Built incrementally by adding file subgraphs. Supports removing a file's
/// subgraph and re-adding it when the file changes.
#[derive(Debug, Default)]
pub struct ScopeGraph {
    nodes: HashMap<ScopeNodeId, ScopeNode>,
    /// Adjacency list: node → outgoing edges.
    edges_from: HashMap<ScopeNodeId, Vec<ScopeEdge>>,
    /// File → set of node IDs belonging to that file.
    file_nodes: HashMap<PathBuf, HashSet<ScopeNodeId>>,
    /// Global export nodes (keyed by symbol name for fast lookup).
    exports: HashMap<String, Vec<ScopeNodeId>>,
    /// Next node ID to allocate.
    next_id: u32,
}

impl ScopeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges_from.values().map(Vec::len).sum()
    }

    /// Add a file's subgraph to the combined graph.
    ///
    /// Node IDs in the file subgraph are remapped to globally unique IDs.
    /// Returns the mapping from old IDs to new IDs.
    pub fn add_file_graph(
        &mut self,
        file_graph: &FileScopeGraph,
    ) -> HashMap<ScopeNodeId, ScopeNodeId> {
        let mut id_map: HashMap<ScopeNodeId, ScopeNodeId> = HashMap::new();

        // Allocate new IDs and insert nodes
        for node in &file_graph.nodes {
            let new_id = ScopeNodeId(self.next_id);
            self.next_id += 1;

            id_map.insert(node.id, new_id);

            let remapped = ScopeNode {
                id: new_id,
                kind: node.kind.clone(),
                file_path: file_graph.file_path.clone(),
                span: node.span,
                symbol_kind: node.symbol_kind,
            };

            self.nodes.insert(new_id, remapped);
            self.file_nodes
                .entry(file_graph.file_path.clone())
                .or_default()
                .insert(new_id);

            // Track exports by symbol for cross-file resolution
            if let ScopeNodeKind::PopSymbol { ref symbol } = node.kind {
                // Check if this node is reachable from an export scope
                if file_graph.export_nodes.contains(&node.id) {
                    self.exports.entry(symbol.clone()).or_default().push(new_id);
                }
            }
        }

        // Remap and insert edges
        for edge in &file_graph.edges {
            if let (Some(&new_src), Some(&new_tgt)) =
                (id_map.get(&edge.source), id_map.get(&edge.target))
            {
                let new_edge = ScopeEdge {
                    id: ScopeEdgeId(self.next_id),
                    source: new_src,
                    target: new_tgt,
                    precedence: edge.precedence,
                };
                self.next_id += 1;
                self.edges_from.entry(new_src).or_default().push(new_edge);
            }
        }

        id_map
    }

    /// Remove all nodes and edges belonging to a file.
    pub fn remove_file(&mut self, path: &Path) {
        if let Some(node_ids) = self.file_nodes.remove(path) {
            for &nid in &node_ids {
                // Remove from exports
                if let Some(node) = self.nodes.get(&nid) {
                    if let ScopeNodeKind::PopSymbol { ref symbol } = node.kind {
                        if let Some(exports) = self.exports.get_mut(symbol) {
                            exports.retain(|&id| id != nid);
                        }
                    }
                }
                self.nodes.remove(&nid);
                self.edges_from.remove(&nid);
            }

            // Remove edges targeting removed nodes
            for edges in self.edges_from.values_mut() {
                edges.retain(|e| !node_ids.contains(&e.target));
            }
        }
    }

    /// Get a node by its ID.
    pub fn get_node(&self, id: ScopeNodeId) -> Option<&ScopeNode> {
        self.nodes.get(&id)
    }

    /// Get all outgoing edges from a node.
    pub fn edges_from(&self, id: ScopeNodeId) -> &[ScopeEdge] {
        self.edges_from.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Find all push-symbol nodes (references) in the graph.
    pub fn push_nodes(&self) -> Vec<&ScopeNode> {
        self.nodes
            .values()
            .filter(|n| matches!(n.kind, ScopeNodeKind::PushSymbol { .. }))
            .collect()
    }

    /// Find all pop-symbol nodes (definitions) matching a symbol name.
    pub fn definitions_for(&self, symbol: &str) -> Vec<&ScopeNode> {
        self.nodes
            .values()
            .filter(|n| matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol: s } if s == symbol))
            .collect()
    }

    /// Resolve all references in the graph using path-stitching.
    ///
    /// Returns a list of resolved references (reference → definition pairs).
    /// Each reference may resolve to zero, one, or multiple definitions.
    pub fn resolve_all(&self) -> Vec<ResolvedReference> {
        let push_nodes: Vec<_> = self.push_nodes();
        let mut results = Vec::new();

        for push_node in push_nodes {
            let resolved = self.resolve_reference(push_node);
            results.extend(resolved);
        }

        results
    }

    /// Resolve a single push-symbol node to its definitions via BFS path-stitching.
    fn resolve_reference(&self, push_node: &ScopeNode) -> Vec<ResolvedReference> {
        let symbol = match &push_node.kind {
            ScopeNodeKind::PushSymbol { symbol } => symbol.clone(),
            _ => return vec![],
        };

        let mut results = Vec::new();

        // BFS from the push node, following edges and matching push/pop symbols
        let mut queue = VecDeque::new();
        let initial = PartialPath {
            symbol_stack: vec![symbol.clone()],
            current_node: push_node.id,
            visited: HashSet::from([push_node.id]),
            start_node: push_node.id,
        };
        queue.push_back(initial);

        let max_depth = 100;
        let mut iterations = 0;

        while let Some(path) = queue.pop_front() {
            iterations += 1;
            if iterations > max_depth * self.node_count().max(1) {
                break; // Safety limit
            }

            // Check if we've found a resolution (empty stack at a pop-symbol node)
            if let Some(target_node) = self.nodes.get(&path.current_node) {
                if path.symbol_stack.is_empty() {
                    if let ScopeNodeKind::PopSymbol { .. } = &target_node.kind {
                        results.push(ResolvedReference {
                            reference_node: push_node.id,
                            definition_node: target_node.id,
                            symbol: symbol.clone(),
                            kind: target_node.symbol_kind,
                            reference_file: push_node.file_path.clone(),
                            definition_file: target_node.file_path.clone(),
                            confidence: 1.0,
                        });
                        continue;
                    }
                }
            }

            // Traverse outgoing edges
            for edge in self.edges_from(path.current_node) {
                if path.visited.contains(&edge.target) {
                    continue;
                }

                let Some(target) = self.nodes.get(&edge.target) else {
                    continue;
                };

                let new_stack = match &target.kind {
                    ScopeNodeKind::PushSymbol { symbol: s } => {
                        let mut stack = path.symbol_stack.clone();
                        stack.push(s.clone());
                        stack
                    }
                    ScopeNodeKind::PopSymbol { symbol: s } => {
                        if path.symbol_stack.last().is_some_and(|top| top == s) {
                            let mut stack = path.symbol_stack.clone();
                            stack.pop();
                            stack
                        } else {
                            continue; // Symbol mismatch — this path is invalid
                        }
                    }
                    ScopeNodeKind::Scope
                    | ScopeNodeKind::Root
                    | ScopeNodeKind::ExportScope
                    | ScopeNodeKind::ImportScope => path.symbol_stack.clone(),
                };

                let mut visited = path.visited.clone();
                visited.insert(edge.target);

                queue.push_back(PartialPath {
                    symbol_stack: new_stack,
                    current_node: edge.target,
                    visited,
                    start_node: path.start_node,
                });
            }

            // Cross-file resolution: if at an import scope with unresolved symbols,
            // try to jump to exported definitions in other files.
            if let Some(node) = self.nodes.get(&path.current_node) {
                if matches!(node.kind, ScopeNodeKind::ImportScope) {
                    if let Some(top_symbol) = path.symbol_stack.last() {
                        if let Some(export_nodes) = self.exports.get(top_symbol) {
                            for &export_id in export_nodes {
                                if path.visited.contains(&export_id) {
                                    continue;
                                }
                                // Pop the symbol since we're matching against the export
                                let mut new_stack = path.symbol_stack.clone();
                                new_stack.pop();

                                let mut visited = path.visited.clone();
                                visited.insert(export_id);

                                queue.push_back(PartialPath {
                                    symbol_stack: new_stack,
                                    current_node: export_id,
                                    visited,
                                    start_node: path.start_node,
                                });
                            }
                        }
                    }
                }
            }
        }

        results
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::similar_names)]
    fn make_file_graph(path: &str, nodes: Vec<ScopeNode>, edges: Vec<ScopeEdge>) -> FileScopeGraph {
        let root = nodes
            .iter()
            .find(|n| matches!(n.kind, ScopeNodeKind::Root))
            .map_or(ScopeNodeId(0), |n| n.id);
        let exports = nodes
            .iter()
            .filter(|n| matches!(n.kind, ScopeNodeKind::ExportScope))
            .map(|n| n.id)
            .collect();
        let imports = nodes
            .iter()
            .filter(|n| matches!(n.kind, ScopeNodeKind::ImportScope))
            .map(|n| n.id)
            .collect();

        FileScopeGraph {
            file_path: PathBuf::from(path),
            nodes,
            edges,
            root_scope: root,
            export_nodes: exports,
            import_nodes: imports,
        }
    }

    #[test]
    fn single_file_resolution() {
        // Simple case: reference `foo` resolves to definition `foo` within same file.
        //
        // Graph: Root → PushSymbol("foo") → Scope → PopSymbol("foo")
        let nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("main.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PushSymbol {
                    symbol: "foo".to_string(),
                },
                file_path: PathBuf::from("main.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::Scope,
                file_path: PathBuf::from("main.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(3),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "foo".to_string(),
                },
                file_path: PathBuf::from("main.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
        ];

        let edges = vec![
            ScopeEdge {
                id: ScopeEdgeId(0),
                source: ScopeNodeId(1),
                target: ScopeNodeId(2),
                precedence: 0,
            },
            ScopeEdge {
                id: ScopeEdgeId(1),
                source: ScopeNodeId(2),
                target: ScopeNodeId(3),
                precedence: 0,
            },
        ];

        let file_graph = make_file_graph("main.rs", nodes, edges);

        let mut sg = ScopeGraph::new();
        sg.add_file_graph(&file_graph);

        let resolved = sg.resolve_all();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].symbol, "foo");
        assert!((resolved[0].confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn cross_file_resolution() {
        // File A references `bar`, file B defines `bar`.
        // A: Root → PushSymbol("bar") → ImportScope
        // B: Root → ExportScope, PopSymbol("bar")

        let file_a_nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("a.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PushSymbol {
                    symbol: "bar".to_string(),
                },
                file_path: PathBuf::from("a.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::ImportScope,
                file_path: PathBuf::from("a.rs"),
                span: None,
                symbol_kind: None,
            },
        ];

        let file_a_edges = vec![ScopeEdge {
            id: ScopeEdgeId(0),
            source: ScopeNodeId(1),
            target: ScopeNodeId(2),
            precedence: 0,
        }];

        let file_b_nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("b.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::ExportScope,
                file_path: PathBuf::from("b.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "bar".to_string(),
                },
                file_path: PathBuf::from("b.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
        ];

        let file_b_edges = vec![ScopeEdge {
            id: ScopeEdgeId(0),
            source: ScopeNodeId(1),
            target: ScopeNodeId(2),
            precedence: 0,
        }];

        // Mark the pop-symbol as reachable from export
        let mut file_b = make_file_graph("b.rs", file_b_nodes, file_b_edges);
        file_b.export_nodes.push(ScopeNodeId(2)); // The PopSymbol is exported

        let file_a = make_file_graph("a.rs", file_a_nodes, file_a_edges);

        let mut sg = ScopeGraph::new();
        sg.add_file_graph(&file_a);
        sg.add_file_graph(&file_b);

        let resolved = sg.resolve_all();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].symbol, "bar");
        assert_eq!(resolved[0].reference_file, PathBuf::from("a.rs"));
        assert_eq!(resolved[0].definition_file, PathBuf::from("b.rs"));
    }

    #[test]
    fn remove_file_clears_nodes() {
        let nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("x.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "x".to_string(),
                },
                file_path: PathBuf::from("x.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
        ];

        let file = make_file_graph("x.rs", nodes, vec![]);

        let mut sg = ScopeGraph::new();
        sg.add_file_graph(&file);
        assert_eq!(sg.node_count(), 2);

        sg.remove_file(Path::new("x.rs"));
        assert_eq!(sg.node_count(), 0);
    }

    #[test]
    fn no_resolution_on_mismatched_symbols() {
        // Reference to `foo` but only `bar` is defined.
        let nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("m.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PushSymbol {
                    symbol: "foo".to_string(),
                },
                file_path: PathBuf::from("m.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::Scope,
                file_path: PathBuf::from("m.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(3),
                kind: ScopeNodeKind::PopSymbol {
                    symbol: "bar".to_string(),
                },
                file_path: PathBuf::from("m.rs"),
                span: None,
                symbol_kind: Some(SymbolKind::Function),
            },
        ];

        let edges = vec![
            ScopeEdge {
                id: ScopeEdgeId(0),
                source: ScopeNodeId(1),
                target: ScopeNodeId(2),
                precedence: 0,
            },
            ScopeEdge {
                id: ScopeEdgeId(1),
                source: ScopeNodeId(2),
                target: ScopeNodeId(3),
                precedence: 0,
            },
        ];

        let file = make_file_graph("m.rs", nodes, edges);

        let mut sg = ScopeGraph::new();
        sg.add_file_graph(&file);

        let resolved = sg.resolve_all();
        assert!(resolved.is_empty(), "Mismatched symbols should not resolve");
    }

    #[test]
    fn multiple_definitions_same_symbol() {
        // Two files both export `helper` — reference should resolve to both.
        let make_exporting_file = |path: &str| {
            let nodes = vec![
                ScopeNode {
                    id: ScopeNodeId(0),
                    kind: ScopeNodeKind::Root,
                    file_path: PathBuf::from(path),
                    span: None,
                    symbol_kind: None,
                },
                ScopeNode {
                    id: ScopeNodeId(1),
                    kind: ScopeNodeKind::ExportScope,
                    file_path: PathBuf::from(path),
                    span: None,
                    symbol_kind: None,
                },
                ScopeNode {
                    id: ScopeNodeId(2),
                    kind: ScopeNodeKind::PopSymbol {
                        symbol: "helper".to_string(),
                    },
                    file_path: PathBuf::from(path),
                    span: None,
                    symbol_kind: Some(SymbolKind::Function),
                },
            ];
            let edges = vec![ScopeEdge {
                id: ScopeEdgeId(0),
                source: ScopeNodeId(1),
                target: ScopeNodeId(2),
                precedence: 0,
            }];
            let mut fg = make_file_graph(path, nodes, edges);
            fg.export_nodes.push(ScopeNodeId(2));
            fg
        };

        let ref_nodes = vec![
            ScopeNode {
                id: ScopeNodeId(0),
                kind: ScopeNodeKind::Root,
                file_path: PathBuf::from("caller.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(1),
                kind: ScopeNodeKind::PushSymbol {
                    symbol: "helper".to_string(),
                },
                file_path: PathBuf::from("caller.rs"),
                span: None,
                symbol_kind: None,
            },
            ScopeNode {
                id: ScopeNodeId(2),
                kind: ScopeNodeKind::ImportScope,
                file_path: PathBuf::from("caller.rs"),
                span: None,
                symbol_kind: None,
            },
        ];
        let ref_edges = vec![ScopeEdge {
            id: ScopeEdgeId(0),
            source: ScopeNodeId(1),
            target: ScopeNodeId(2),
            precedence: 0,
        }];

        let mut sg = ScopeGraph::new();
        sg.add_file_graph(&make_exporting_file("mod_a.rs"));
        sg.add_file_graph(&make_exporting_file("mod_b.rs"));
        sg.add_file_graph(&make_file_graph("caller.rs", ref_nodes, ref_edges));

        let resolved = sg.resolve_all();
        assert_eq!(resolved.len(), 2, "Should resolve to both definitions");
        let def_files: HashSet<_> = resolved.iter().map(|r| r.definition_file.clone()).collect();
        assert!(def_files.contains(&PathBuf::from("mod_a.rs")));
        assert!(def_files.contains(&PathBuf::from("mod_b.rs")));
    }
}
