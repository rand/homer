use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocCommentData, DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport,
    ResolutionTier, Result, SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{ScopeGraphBuilder, hash_string, node_range, node_text};

#[derive(Debug)]
pub struct LeanSupport;

impl LanguageSupport for LeanSupport {
    fn id(&self) -> &'static str {
        "lean"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["lean"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_lean4::language()
    }

    fn build_scope_graph(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<Option<FileScopeGraph>> {
        let mut builder = ScopeGraphBuilder::new(path);
        let root = builder.root();
        let mut exported_defs = Vec::new();

        scope_walk(
            tree.root_node(),
            source,
            root,
            &mut builder,
            &mut exported_defs,
            false, // not inside a private declaration
        );

        for def_id in &exported_defs {
            builder.mark_exported(*def_id);
        }

        Ok(Some(builder.build()))
    }

    fn extract_heuristic(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<HeuristicGraph> {
        let mut defs = Vec::new();
        let mut calls = Vec::new();
        let mut imports = Vec::new();
        let mut context: Vec<String> = Vec::new();

        walk_lean_node(
            tree.root_node(),
            source,
            path,
            &mut context,
            &mut defs,
            &mut calls,
            &mut imports,
        );

        Ok(HeuristicGraph {
            file_path: path.to_path_buf(),
            definitions: defs,
            calls,
            imports,
        })
    }
}

// ── Heuristic extraction ─────────────────────────────────────────────
//
// Lean 4's tree-sitter grammar uses a flat AST inside `module`:
// - `namespace Foo` is a marker node (not a container)
// - `end Foo` is a sibling marker that closes the namespace
// - `private` is a sibling node preceding the definition it modifies
//
// We iterate children sequentially, tracking namespace context via a stack.

#[allow(clippy::too_many_lines)]
fn walk_lean_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    if node.kind() == "module" {
        // Walk children of module with namespace tracking
        walk_module_children(node, source, path, context, defs, calls, imports);
        return;
    }

    match node.kind() {
        "definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_lean_doc_comment(node, source);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });
            }
        }
        "constant" | "axiom" | "opaque" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_lean_doc_comment(node, source);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Constant,
                    span: node_range(node),
                    doc_comment: doc,
                });
            }
        }
        "structure" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_lean_doc_comment(node, source);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: doc,
                });

                // Walk fields inside structure scope
                context.push(name);
                walk_lean_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return; // Skip default child walk
            }
        }
        "inductive" | "class_inductive" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_lean_doc_comment(node, source);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: doc,
                });

                // Walk constructors
                context.push(name);
                walk_lean_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "constructor" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: None,
                });
            }
        }
        "import" => {
            if let Some(module_node) = node.child_by_field_name("module") {
                let module_name = node_text(module_node, source).to_string();
                imports.push(HeuristicImport {
                    from_path: path.to_path_buf(),
                    imported_name: module_name,
                    target_path: None,
                    confidence: 0.9,
                });
            }
        }
        "structure_field" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Field,
                    span: node_range(node),
                    doc_comment: None,
                });
            }
        }
        "application" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let callee_name = node_text(name_node, source).to_string();
                let caller = if context.is_empty() {
                    "<module>".to_string()
                } else {
                    context.join(".")
                };
                calls.push(HeuristicCall {
                    caller,
                    callee_name,
                    span: node_range(node),
                    confidence: 0.6,
                });
            }
        }
        _ => {}
    }

    walk_lean_children(node, source, path, context, defs, calls, imports);
}

/// Walk children of a `module` node with flat namespace/end tracking.
fn walk_module_children(
    module_node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    let mut cursor = module_node.walk();
    for child in module_node.children(&mut cursor) {
        match child.kind() {
            "namespace" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(name_node, source).to_string();
                    let qname = dotted_name(context, &name);

                    defs.push(HeuristicDef {
                        name: name.clone(),
                        qualified_name: qname,
                        kind: SymbolKind::Module,
                        span: node_range(child),
                        doc_comment: None,
                    });

                    context.push(name);
                }
            }
            "end" => {
                // `end Foo` pops the most recent namespace
                context.pop();
            }
            _ => {
                walk_lean_node(child, source, path, context, defs, calls, imports);
            }
        }
    }
}

fn walk_lean_children(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_lean_node(child, source, path, context, defs, calls, imports);
    }
}

/// Extract Lean 4 doc comment (`/-- ... -/`) from the preceding sibling.
fn extract_lean_doc_comment(node: tree_sitter::Node<'_>, source: &str) -> Option<DocCommentData> {
    // Walk backwards to find a comment preceding this node
    // In Lean 4, doc comments are `/-- ... -/`
    // The tree-sitter grammar may attach them as `comment` nodes or as part
    // of a `declaration` wrapper. Check the node's parent for `declaration`
    // which has an `attributes` field, or look for a preceding comment sibling.
    let check_node = if let Some(parent) = node.parent() {
        if parent.kind() == "declaration" || parent.kind() == "_declaration" {
            parent
        } else {
            node
        }
    } else {
        node
    };

    let prev = check_node.prev_sibling()?;
    if prev.kind() != "comment" {
        return None;
    }

    let text = node_text(prev, source);
    // Lean 4 doc comments: /-- text -/
    let inner = text.strip_prefix("/--")?;
    let inner = inner.strip_suffix("-/")?;
    let cleaned = inner.trim().to_string();

    if cleaned.is_empty() {
        return None;
    }

    let content_hash = hash_string(&cleaned);
    Some(DocCommentData {
        text: cleaned,
        content_hash,
        style: DocStyle::LeanDoc,
    })
}

/// Check if a declaration has a `private` modifier.
///
/// In Lean 4's flat AST, `private` is a preceding sibling node:
/// ```text
/// module
///   private     ← sibling
///   definition  ← this node
/// ```
fn is_private(node: tree_sitter::Node<'_>, _source: &str) -> bool {
    // Check if the previous sibling is a `private` keyword node
    if let Some(prev) = node.prev_sibling() {
        if prev.kind() == "private" {
            return true;
        }
    }
    false
}

fn dotted_name(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_string()
    } else {
        format!("{}.{name}", context.join("."))
    }
}

// ── Scope graph construction ─────────────────────────────────────────

use crate::scope_graph::ScopeNodeId;

fn scope_walk(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    if node.kind() == "module" {
        // Module level uses flat namespace/end tracking
        scope_walk_module(node, source, scope, builder, exported);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, exported, in_private);
    }
}

/// Walk children of a `module` node with flat namespace/end and private tracking.
fn scope_walk_module(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let mut scope_stack: Vec<ScopeNodeId> = vec![scope];
    let mut next_private = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let current_scope = *scope_stack.last().unwrap_or(&scope);

        match child.kind() {
            "namespace" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(name_node, source);
                    builder.add_definition(
                        current_scope,
                        name,
                        Some(node_range(name_node)),
                        Some(SymbolKind::Module),
                    );
                    let ns_scope = builder.add_scope(current_scope, Some(node_range(child)));
                    scope_stack.push(ns_scope);
                }
                next_private = false;
            }
            "end" => {
                if scope_stack.len() > 1 {
                    scope_stack.pop();
                }
                next_private = false;
            }
            "private" => {
                next_private = true;
            }
            _ => {
                scope_dispatch(
                    child,
                    source,
                    current_scope,
                    builder,
                    exported,
                    next_private,
                );
                // Reset private flag after processing the next declaration
                if child.kind() != "comment" {
                    next_private = false;
                }
            }
        }
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    match node.kind() {
        "definition" => {
            scope_definition(node, source, scope, builder, exported, in_private);
        }
        "constant" | "axiom" | "opaque" => {
            scope_constant(node, source, scope, builder, exported, in_private);
        }
        "structure" => {
            scope_structure(node, source, scope, builder, exported, in_private);
        }
        "inductive" | "class_inductive" => {
            scope_inductive(node, source, scope, builder, exported, in_private);
        }
        "namespace" => {
            scope_namespace(node, source, scope, builder, exported, in_private);
        }
        "section" => {
            let section_scope = builder.add_scope(scope, Some(node_range(node)));
            scope_walk(node, source, section_scope, builder, exported, in_private);
        }
        "import" => {
            scope_import(node, source, scope, builder);
        }
        "open" => {
            scope_open(node, source, scope, builder);
        }
        "constructor" => {
            scope_constructor(node, source, scope, builder, exported, in_private);
        }
        "structure_field" => {
            scope_field(node, source, scope, builder);
        }
        "application" => {
            scope_application(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported, in_private);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported, in_private);
        }
    }
}

fn scope_definition(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);

    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Function),
    );
    // In Lean 4, definitions are exported by default unless `private`
    if !in_private {
        exported.push(def_id);
    }

    // Create scope for body
    let def_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_walk(node, source, def_scope, builder, exported, in_private);
}

fn scope_constant(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);
    let is_priv = in_private || is_private(node, source);

    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Constant),
    );
    if !is_priv {
        exported.push(def_id);
    }
}

fn scope_structure(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);
    let is_priv = in_private || is_private(node, source);

    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Type),
    );
    if !is_priv {
        exported.push(def_id);
    }

    let struct_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_walk(node, source, struct_scope, builder, exported, in_private);
}

fn scope_inductive(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);
    let is_priv = in_private || is_private(node, source);

    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Type),
    );
    if !is_priv {
        exported.push(def_id);
    }

    let ind_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_walk(node, source, ind_scope, builder, exported, in_private);
}

fn scope_namespace(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);

    builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Module),
    );

    let ns_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_walk(node, source, ns_scope, builder, exported, in_private);
}

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(module_node) = node.child_by_field_name("module") else {
        return;
    };
    let module_name = node_text(module_node, source);

    let import_scope = builder.add_import_scope();
    builder.add_import_reference(import_scope, module_name, Some(node_range(node)));

    // Bind the last segment as a local name
    let local_name = module_name.rsplit('.').next().unwrap_or(module_name);
    builder.add_definition(
        scope,
        local_name,
        Some(node_range(node)),
        Some(SymbolKind::Module),
    );
}

fn scope_open(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // `open Foo` — reference to the opened namespace
    if let Some(ns_node) = node.child_by_field_name("namespace") {
        let name = node_text(ns_node, source);
        builder.add_reference(
            scope,
            name,
            Some(node_range(ns_node)),
            Some(SymbolKind::Module),
        );
    }
}

fn scope_constructor(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    in_private: bool,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, source);

    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Function),
    );
    if !in_private {
        exported.push(def_id);
    }
}

fn scope_field(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(name_node, source);
        builder.add_definition(
            scope,
            name,
            Some(node_range(name_node)),
            Some(SymbolKind::Field),
        );
    }
}

fn scope_application(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(name_node, source);
        builder.add_reference(
            scope,
            name,
            Some(node_range(name_node)),
            Some(SymbolKind::Function),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_lean(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_lean4::language()).unwrap();
        parser.parse(source, None).unwrap()
    }

    // ── Heuristic tests ──────────────────────────────────────────

    #[test]
    fn extracts_definitions() {
        let source =
            "def greet (name : String) : String := s!\"Hello, {name}!\"\n\ndef helper := 42\n";
        let tree = parse_lean(source);
        let graph = LeanSupport
            .extract_heuristic(&tree, source, Path::new("Main.lean"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "greet" && d.kind == SymbolKind::Function),
            "Should find greet, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "helper" && d.kind == SymbolKind::Function),
            "Should find helper, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_types() {
        let source = "structure Point where\n  x : Float\n  y : Float\n\ninductive Color where\n  | red\n  | green\n  | blue\n";
        let tree = parse_lean(source);
        let graph = LeanSupport
            .extract_heuristic(&tree, source, Path::new("Types.lean"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Point" && d.kind == SymbolKind::Type),
            "Should detect Point as Type, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Color" && d.kind == SymbolKind::Type),
            "Should detect Color as Type, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_imports() {
        let source = "import Lean\nimport Mathlib.Topology\n";
        let tree = parse_lean(source);
        let graph = LeanSupport
            .extract_heuristic(&tree, source, Path::new("Main.lean"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should find at least 2 imports, got: {:?}",
            graph.imports
        );
        assert!(graph.imports.iter().any(|i| i.imported_name == "Lean"));
        assert!(
            graph
                .imports
                .iter()
                .any(|i| i.imported_name == "Mathlib.Topology")
        );
    }

    #[test]
    fn extracts_namespace() {
        let source = "namespace Foo\n\ndef bar := 1\n\nend Foo\n";
        let tree = parse_lean(source);
        let graph = LeanSupport
            .extract_heuristic(&tree, source, Path::new("Ns.lean"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Foo" && d.kind == SymbolKind::Module),
            "Should find namespace Foo, got: {:?}",
            graph.definitions
        );
        // bar should be qualified under Foo
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.qualified_name == "Foo.bar"),
            "bar should be qualified as Foo.bar, got: {:?}",
            graph.definitions
        );
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::ScopeNodeKind;

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_lean(source);
        LeanSupport
            .build_scope_graph(&tree, source, Path::new("test.lean"))
            .unwrap()
            .expect("should produce a scope graph")
    }

    fn pop_symbols(graph: &FileScopeGraph) -> Vec<&str> {
        graph
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                ScopeNodeKind::PopSymbol { symbol } => Some(symbol.as_str()),
                _ => None,
            })
            .collect()
    }

    fn push_symbols(graph: &FileScopeGraph) -> Vec<&str> {
        graph
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                ScopeNodeKind::PushSymbol { symbol } => Some(symbol.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn scope_graph_definitions() {
        let sg = build_scope(
            "def greet (name : String) : String := s!\"Hello, {name}!\"\n\ndef helper := 42\n",
        );
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(
            defs.contains(&"helper"),
            "Should have helper, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_public_by_default() {
        let sg = build_scope("def foo := 1\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "foo")
                })
            }),
            "Lean defs should be exported by default"
        );
    }

    #[test]
    fn scope_graph_private_suppression() {
        let sg = build_scope("private def secret := 42\n\ndef visible := 1\n");
        // visible should be exported
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "visible")
                })
            }),
            "Non-private def should be exported"
        );
        // secret should NOT be exported
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "secret")
                })
            }),
            "Private def should NOT be exported"
        );
    }

    #[test]
    fn scope_graph_namespace_scopes() {
        let sg = build_scope("namespace Foo\n\ndef bar := 1\n\nend Foo\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have namespace Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"bar"),
            "Should have def bar inside namespace, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_imports() {
        let sg = build_scope("import Lean\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Lean"),
            "Should bind import Lean, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Lean"),
            "Should reference import Lean, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_open() {
        let sg = build_scope("open Nat\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Nat"),
            "Should reference opened Nat, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_structure_fields() {
        let sg = build_scope("structure Point where\n  x : Float\n  y : Float\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Point"),
            "Should have struct Point, got: {defs:?}"
        );
        assert!(defs.contains(&"x"), "Should have field x, got: {defs:?}");
        assert!(defs.contains(&"y"), "Should have field y, got: {defs:?}");
    }

    #[test]
    fn scope_graph_inductive_constructors() {
        let sg = build_scope("inductive Color where\n  | red\n  | green\n  | blue\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Color"),
            "Should have inductive Color, got: {defs:?}"
        );
        assert!(
            defs.contains(&"red"),
            "Should have constructor red, got: {defs:?}"
        );
        assert!(
            defs.contains(&"green"),
            "Should have constructor green, got: {defs:?}"
        );
    }
}
