use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, dotted_name, extract_doc_comment_above, find_child_by_kind,
    node_range, node_text,
};

#[derive(Debug)]
pub struct SwiftSupport;

impl LanguageSupport for SwiftSupport {
    fn id(&self) -> &'static str {
        "swift"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_swift::LANGUAGE.into()
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

        walk_swift_node(
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

#[allow(clippy::too_many_lines)]
fn walk_swift_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::SwiftDoc, "///");

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "init_declaration" => {
            let qname = dotted_name(context, "init");
            let doc = extract_doc_comment_above(node, source, DocStyle::SwiftDoc, "///");

            defs.push(HeuristicDef {
                name: "init".to_string(),
                qualified_name: qname.clone(),
                kind: SymbolKind::Function,
                span: node_range(node),
                doc_comment: doc,
            });

            if let Some(body) = child_by_field(node, "body") {
                extract_calls_recursive(body, source, &qname, calls);
            }
        }
        "class_declaration" => {
            // Covers struct, class, and enum (distinguished by declaration_kind field)
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::SwiftDoc, "///"),
                });

                context.push(name);
                walk_swift_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "protocol_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::SwiftDoc, "///"),
                });

                context.push(name);
                walk_swift_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "protocol_function_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::SwiftDoc, "///"),
                });
            }
        }
        "import_declaration" => {
            // `import Foundation` — gather the identifier children
            let import_name = collect_import_name(node, source);
            if !import_name.is_empty() {
                imports.push(HeuristicImport {
                    from_path: path.to_path_buf(),
                    imported_name: import_name,
                    target_path: None,
                    confidence: 0.9,
                });
            }
        }
        _ => {}
    }

    walk_swift_children(node, source, path, context, defs, calls, imports);
}

fn walk_swift_children(
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
        walk_swift_node(child, source, path, context, defs, calls, imports);
    }
}

/// Collect the imported module name from an `import_declaration` node.
///
/// Swift imports look like `import Foundation` or `import UIKit.UIView`.
/// The module name is built from `identifier` children containing `simple_identifier`.
fn collect_import_name(node: tree_sitter::Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    let parts: Vec<&str> = node
        .children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .map(|c| node_text(c, source))
        .collect();

    parts.join(".")
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "call_expression" {
        let target = call_target_name(node, source);
        if let Some(name) = target {
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: name,
                span: node_range(node),
                confidence: 0.7,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls_recursive(child, source, caller, calls);
    }
}

/// Extract the callee name from a `call_expression`.
///
/// Tries `child_by_field(node, "name")` first, then falls back to the first
/// `simple_identifier` or `navigation_expression` child.
fn call_target_name(node: tree_sitter::Node<'_>, source: &str) -> Option<String> {
    // Try the "name" field first (not all grammars set it)
    if let Some(name_node) = child_by_field(node, "name") {
        return Some(node_text(name_node, source).to_string());
    }

    // Fallback: first child that looks like a callable target
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "simple_identifier" | "navigation_expression" => {
                return Some(node_text(child, source).to_string());
            }
            _ => {}
        }
    }
    None
}

// ── Scope graph construction ─────────────────────────────────────────

use crate::scope_graph::ScopeNodeId;

fn scope_walk(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, exported);
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    match node.kind() {
        "function_declaration" => {
            scope_func_decl(node, source, scope, builder, exported);
        }
        "init_declaration" => {
            scope_init_decl(node, source, scope, builder, exported);
        }
        "class_declaration" => {
            scope_class_decl(node, source, scope, builder, exported);
        }
        "protocol_declaration" => {
            scope_protocol_decl(node, source, scope, builder, exported);
        }
        "protocol_function_declaration" => {
            scope_protocol_func(node, source, scope, builder);
        }
        "import_declaration" => {
            scope_import(node, source, scope, builder);
        }
        "call_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported);
        }
    }
}

/// Check whether a declaration has `private` or `fileprivate` visibility.
///
/// In Swift, top-level declarations without an explicit access modifier are
/// `internal` by default and should be exported for cross-file resolution.
/// Only `private` and `fileprivate` restrict visibility to the file.
fn is_private(node: tree_sitter::Node<'_>) -> bool {
    let Some(mods) = find_child_by_kind(node, "modifiers") else {
        return false;
    };
    let mut cursor = mods.walk();
    mods.children(&mut cursor).any(|c| {
        if c.kind() == "visibility_modifier" {
            let mut inner = c.walk();
            c.children(&mut inner)
                .any(|v| v.kind() == "private" || v.kind() == "fileprivate")
        } else {
            false
        }
    })
}

fn scope_func_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Function),
    );
    if !is_private(node) {
        exported.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_params(node, source, func_scope, builder);
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, func_scope, builder, exported);
    }
}

fn scope_init_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let def_id = builder.add_definition(
        scope,
        "init",
        Some(node_range(node)),
        Some(SymbolKind::Function),
    );
    if !is_private(node) {
        exported.push(def_id);
    }

    let init_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_params(node, source, init_scope, builder);
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, init_scope, builder, exported);
    }
}

fn scope_class_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Type),
    );
    if !is_private(node) {
        exported.push(def_id);
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, class_scope, builder, exported);
    }
}

fn scope_protocol_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Type),
    );
    if !is_private(node) {
        exported.push(def_id);
    }

    let proto_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, proto_scope, builder, exported);
    }
}

fn scope_protocol_func(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Function),
    );
    // Protocol function declarations have no body scope
}

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let import_name = collect_import_name(node, source);
    if import_name.is_empty() {
        return;
    }

    let import_scope = builder.add_import_scope();
    builder.add_import_reference(import_scope, &import_name, Some(node_range(node)));
    builder.add_definition(
        scope,
        &import_name,
        Some(node_range(node)),
        Some(SymbolKind::Module),
    );
}

fn scope_call(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(name) = call_target_name(node, source) else {
        return;
    };
    // For dotted calls like `obj.method()`, reference the last segment
    let ref_name = name.rsplit('.').next().unwrap_or(&name);
    builder.add_reference(
        scope,
        ref_name,
        Some(node_range(node)),
        Some(SymbolKind::Function),
    );
}

/// Extract parameter definitions from `parameter` children of a function/init node.
fn scope_params(
    func_node: tree_sitter::Node<'_>,
    source: &str,
    func_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = func_node.walk();
    for child in func_node.children(&mut cursor) {
        if child.kind() == "parameter" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    func_scope,
                    name,
                    Some(node_range(name_node)),
                    Some(SymbolKind::Variable),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_swift(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_swift::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    // ── Heuristic extraction tests ──────────────────────────────────

    #[test]
    fn extracts_function_and_struct() {
        let source = "/// Greets someone.\nfunc greet(name: String) {\n    print(name)\n}\n\nstruct Point {\n    var x: Int\n    var y: Int\n}\n";
        let tree = parse_swift(source);
        let graph = SwiftSupport
            .extract_heuristic(&tree, source, Path::new("main.swift"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "greet" && d.kind == SymbolKind::Function),
            "Should find greet function, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Point" && d.kind == SymbolKind::Type),
            "Should find Point struct, got: {:?}",
            graph.definitions
        );
        assert!(
            !graph.calls.is_empty(),
            "Should extract calls from greet body"
        );
    }

    #[test]
    fn extracts_class_and_protocol() {
        let source = "class Animal {\n    func speak() {}\n}\n\nprotocol Describable {\n    func describe() -> String\n}\n";
        let tree = parse_swift(source);
        let graph = SwiftSupport
            .extract_heuristic(&tree, source, Path::new("types.swift"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Animal" && d.kind == SymbolKind::Type),
            "Should find Animal class, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Describable" && d.kind == SymbolKind::Type),
            "Should find Describable protocol, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "speak" && d.kind == SymbolKind::Function),
            "Should find speak method, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_enum() {
        let source = "enum Direction {\n    case north\n    case south\n}\n";
        let tree = parse_swift(source);
        let graph = SwiftSupport
            .extract_heuristic(&tree, source, Path::new("enums.swift"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Direction" && d.kind == SymbolKind::Type),
            "Should find Direction enum, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_imports() {
        let source = "import Foundation\nimport UIKit\n\nfunc main() {}\n";
        let tree = parse_swift(source);
        let graph = SwiftSupport
            .extract_heuristic(&tree, source, Path::new("app.swift"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should find at least 2 imports, got: {:?}",
            graph.imports
        );
        assert!(
            graph
                .imports
                .iter()
                .any(|i| i.imported_name == "Foundation"),
            "Should import Foundation, got: {:?}",
            graph.imports
        );
    }

    // ── Scope graph tests ───────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_swift(source);
        SwiftSupport
            .build_scope_graph(&tree, source, Path::new("test.swift"))
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
    fn scope_graph_function() {
        let sg = build_scope("func greet(name: String) {}\nfunc helper() {}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(
            defs.contains(&"helper"),
            "Should have helper, got: {defs:?}"
        );
        assert!(
            defs.contains(&"name"),
            "Should have param name, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_struct() {
        let sg = build_scope("struct Point {\n    var x: Int\n    var y: Int\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Point"),
            "Should have struct Point, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_class_with_visibility() {
        let sg = build_scope(
            "public class Visible {}\nprivate class Hidden {}\nclass DefaultVisible {}\n",
        );
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Visible"),
            "Should have Visible, got: {defs:?}"
        );
        assert!(
            defs.contains(&"Hidden"),
            "Should have Hidden, got: {defs:?}"
        );
        assert!(
            defs.contains(&"DefaultVisible"),
            "Should have DefaultVisible, got: {defs:?}"
        );

        // Visible and DefaultVisible should be exported, Hidden should not
        let exported_names: Vec<&str> = sg
            .export_nodes
            .iter()
            .filter_map(|&id| {
                sg.nodes.iter().find_map(|n| {
                    if n.id == id {
                        match &n.kind {
                            ScopeNodeKind::PopSymbol { symbol } => Some(symbol.as_str()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
            })
            .collect();

        assert!(
            exported_names.contains(&"Visible"),
            "public class should be exported, got: {exported_names:?}"
        );
        assert!(
            exported_names.contains(&"DefaultVisible"),
            "default (internal) class should be exported, got: {exported_names:?}"
        );
        assert!(
            !exported_names.contains(&"Hidden"),
            "private class should NOT be exported, got: {exported_names:?}"
        );
    }

    #[test]
    fn scope_graph_protocol() {
        let sg = build_scope("protocol Drawable {\n    func draw()\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Drawable"),
            "Should have protocol Drawable, got: {defs:?}"
        );
        assert!(
            defs.contains(&"draw"),
            "Should have protocol method draw, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_enum() {
        let sg = build_scope("enum Color {\n    case red\n    case green\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Color"),
            "Should have enum Color, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_init_declaration() {
        let sg = build_scope("class Foo {\n    init(value: Int) {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
        assert!(defs.contains(&"init"), "Should have init, got: {defs:?}");
        assert!(
            defs.contains(&"value"),
            "Should have init param value, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_import() {
        let sg = build_scope("import Foundation\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foundation"),
            "Import should bind Foundation, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Foundation"),
            "Import should reference Foundation, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_call_expression() {
        let sg = build_scope("func foo() {}\nfunc bar() { foo() }\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should reference foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "func helper() {}\nfunc run() { helper() }\n";
        let sg = build_scope(source);
        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();
        assert!(
            resolved.iter().any(|r| r.symbol == "helper"),
            "helper() should resolve, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_access_control() {
        let sg = build_scope(
            "func publicFunc() {}\nprivate func privateFunc() {}\nfileprivate func fileFunc() {}\n",
        );

        let exported_names: Vec<&str> = sg
            .export_nodes
            .iter()
            .filter_map(|&id| {
                sg.nodes.iter().find_map(|n| {
                    if n.id == id {
                        match &n.kind {
                            ScopeNodeKind::PopSymbol { symbol } => Some(symbol.as_str()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
            })
            .collect();

        assert!(
            exported_names.contains(&"publicFunc"),
            "publicFunc should be exported (default internal), got: {exported_names:?}"
        );
        assert!(
            !exported_names.contains(&"privateFunc"),
            "privateFunc should NOT be exported, got: {exported_names:?}"
        );
        assert!(
            !exported_names.contains(&"fileFunc"),
            "fileprivate func should NOT be exported, got: {exported_names:?}"
        );
    }
}
