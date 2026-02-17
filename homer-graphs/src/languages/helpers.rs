use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::scope_graph::{
    FileScopeGraph, ScopeEdge, ScopeEdgeId, ScopeNode, ScopeNodeId, ScopeNodeKind,
};
use crate::{DocCommentData, DocStyle, SymbolKind, TextRange};

/// Extract the source text for a tree-sitter node.
pub fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// Find the first child with a specific kind.
pub fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Find a child by field name.
pub fn child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

/// Extract a doc comment from comment nodes immediately preceding a definition.
pub fn extract_doc_comment_above(
    node: Node<'_>,
    source: &str,
    style: DocStyle,
    prefix: &str,
) -> Option<DocCommentData> {
    let mut comments = Vec::new();
    let mut current = node;

    // Walk backwards through siblings collecting comment lines
    while let Some(prev) = current.prev_sibling() {
        if prev.kind() == "line_comment" || prev.kind() == "comment" {
            let text = node_text(prev, source);
            if text.starts_with(prefix) {
                let stripped = text.strip_prefix(prefix).unwrap_or(text).trim();
                comments.push(stripped.to_string());
                current = prev;
                continue;
            }
        }
        break;
    }

    if comments.is_empty() {
        return None;
    }

    // Reverse since we collected bottom-to-top
    comments.reverse();
    let text = comments.join("\n");
    let content_hash = hash_string(&text);

    Some(DocCommentData {
        text,
        content_hash,
        style,
    })
}

/// Extract a block doc comment (/** ... */) from the preceding sibling.
pub fn extract_block_doc_comment(
    node: Node<'_>,
    source: &str,
    style: DocStyle,
) -> Option<DocCommentData> {
    let prev = node.prev_sibling()?;
    if prev.kind() != "comment" && prev.kind() != "block_comment" {
        return None;
    }

    let text = node_text(prev, source);
    if !text.starts_with("/**") {
        return None;
    }

    // Strip /** prefix and */ suffix, clean up * at start of lines
    let inner = text
        .strip_prefix("/**")
        .unwrap_or(text)
        .strip_suffix("*/")
        .unwrap_or(text)
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("* ")
                .or(trimmed.strip_prefix('*'))
                .unwrap_or(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if inner.is_empty() {
        return None;
    }

    let content_hash = hash_string(&inner);
    Some(DocCommentData {
        text: inner,
        content_hash,
        style,
    })
}

/// Build a qualified name from a module context stack using `::` separator (Rust).
pub fn qualified_name(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", context.join("::"))
    }
}

/// Build a qualified name from a module context stack using `.` separator (most languages).
pub fn dotted_name(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_string()
    } else {
        format!("{}.{name}", context.join("."))
    }
}

/// Convert a tree-sitter node to a `TextRange`.
pub fn node_range(node: Node<'_>) -> TextRange {
    node.range().into()
}

/// Simple string hash for content dedup.
pub fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ── Scope Graph Builder ──────────────────────────────────────────────

/// Builder for constructing `FileScopeGraph` instances from tree-sitter ASTs.
///
/// Manages node/edge ID allocation and provides helpers for common scope graph
/// patterns (definitions, references, imports, exports).
pub struct ScopeGraphBuilder {
    file_path: PathBuf,
    nodes: Vec<ScopeNode>,
    edges: Vec<ScopeEdge>,
    export_node_ids: Vec<ScopeNodeId>,
    import_node_ids: Vec<ScopeNodeId>,
    root_scope: ScopeNodeId,
    next_node_id: u32,
    next_edge_id: u32,
}

impl ScopeGraphBuilder {
    /// Create a new builder with a root scope for the given file.
    pub fn new(file_path: &Path) -> Self {
        let root_id = ScopeNodeId(0);
        let root = ScopeNode {
            id: root_id,
            kind: ScopeNodeKind::Root,
            file_path: file_path.to_path_buf(),
            span: None,
            symbol_kind: None,
        };
        Self {
            file_path: file_path.to_path_buf(),
            nodes: vec![root],
            edges: vec![],
            export_node_ids: vec![],
            import_node_ids: vec![],
            root_scope: root_id,
            next_node_id: 1,
            next_edge_id: 0,
        }
    }

    pub fn root(&self) -> ScopeNodeId {
        self.root_scope
    }

    /// Add a child scope linked to its parent (for LEGB-style lookup chains).
    pub fn add_scope(&mut self, parent: ScopeNodeId, span: Option<TextRange>) -> ScopeNodeId {
        let id = self.alloc_id();
        self.push_node(id, ScopeNodeKind::Scope, span, None);
        self.add_edge(id, parent, 1); // child → parent for name lookup
        id
    }

    /// Add a definition (`PopSymbol`) reachable from the given scope.
    pub fn add_definition(
        &mut self,
        scope: ScopeNodeId,
        symbol: &str,
        span: Option<TextRange>,
        kind: Option<SymbolKind>,
    ) -> ScopeNodeId {
        let id = self.alloc_id();
        self.push_node(id, ScopeNodeKind::PopSymbol { symbol: symbol.to_string() }, span, kind);
        self.add_edge(scope, id, 0); // scope → definition
        id
    }

    /// Add a reference (`PushSymbol`) that looks up in the given scope.
    pub fn add_reference(
        &mut self,
        scope: ScopeNodeId,
        symbol: &str,
        span: Option<TextRange>,
        kind: Option<SymbolKind>,
    ) -> ScopeNodeId {
        let id = self.alloc_id();
        self.push_node(id, ScopeNodeKind::PushSymbol { symbol: symbol.to_string() }, span, kind);
        self.add_edge(id, scope, 0); // reference → scope (lookup direction)
        id
    }

    /// Add an import scope boundary (for cross-file resolution).
    pub fn add_import_scope(&mut self) -> ScopeNodeId {
        let id = self.alloc_id();
        self.push_node(id, ScopeNodeKind::ImportScope, None, None);
        self.import_node_ids.push(id);
        id
    }

    /// Add an import reference: a `PushSymbol` that points to an `ImportScope`.
    pub fn add_import_reference(
        &mut self,
        import_scope: ScopeNodeId,
        symbol: &str,
        span: Option<TextRange>,
    ) -> ScopeNodeId {
        let id = self.alloc_id();
        self.push_node(
            id,
            ScopeNodeKind::PushSymbol { symbol: symbol.to_string() },
            span,
            None,
        );
        self.add_edge(id, import_scope, 0);
        id
    }

    /// Mark a `PopSymbol` node as exported (available for cross-file resolution).
    pub fn mark_exported(&mut self, node_id: ScopeNodeId) {
        self.export_node_ids.push(node_id);
    }

    /// Add a raw edge between two nodes.
    pub fn add_edge(&mut self, source: ScopeNodeId, target: ScopeNodeId, precedence: u8) {
        let id = ScopeEdgeId(self.next_edge_id);
        self.next_edge_id += 1;
        self.edges.push(ScopeEdge {
            id,
            source,
            target,
            precedence,
        });
    }

    /// Consume the builder and produce a `FileScopeGraph`.
    pub fn build(self) -> FileScopeGraph {
        FileScopeGraph {
            file_path: self.file_path,
            nodes: self.nodes,
            edges: self.edges,
            root_scope: self.root_scope,
            export_nodes: self.export_node_ids,
            import_nodes: self.import_node_ids,
        }
    }

    fn alloc_id(&mut self) -> ScopeNodeId {
        let id = ScopeNodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    fn push_node(
        &mut self,
        id: ScopeNodeId,
        kind: ScopeNodeKind,
        span: Option<TextRange>,
        symbol_kind: Option<SymbolKind>,
    ) {
        self.nodes.push(ScopeNode {
            id,
            kind,
            file_path: self.file_path.clone(),
            span,
            symbol_kind,
        });
    }
}
