use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, extract_doc_comment_above, find_child_by_kind, node_range, node_text,
};

#[derive(Debug)]
pub struct ZigSupport;

impl LanguageSupport for ZigSupport {
    fn id(&self) -> &'static str {
        "zig"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zig"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_zig::LANGUAGE.into()
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

        walk_zig_node(
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

fn walk_zig_node(
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
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::ZigDoc, "///");

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                if let Some(body) = node.child_by_field_name("body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "variable_declaration" => {
            if let Some(name_node) = find_child_by_kind(node, "identifier") {
                let name = node_text(name_node, source).to_string();
                let decl_text = node_text(node, source);
                let is_const = is_const_decl(decl_text);

                // Check if this is a type definition (struct/enum/union/error_set)
                let kind = if has_type_value(node) {
                    SymbolKind::Type
                } else if is_const {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };

                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::ZigDoc, "///");

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind,
                    span: node_range(node),
                    doc_comment: doc,
                });

                // Check for @import
                if let Some(import_path) = extract_import_path(node, source) {
                    imports.push(HeuristicImport {
                        from_path: path.to_path_buf(),
                        imported_name: import_path.to_string(),
                        target_path: None,
                        confidence: 0.9,
                    });
                }
            }
        }
        "test_declaration" => {
            // Test blocks: test "name" { ... }
            let name = extract_test_name(node, source).unwrap_or_else(|| "test".to_string());
            let qname = dotted_name(context, &name);

            defs.push(HeuristicDef {
                name,
                qualified_name: qname,
                kind: SymbolKind::Function,
                span: node_range(node),
                doc_comment: None,
            });
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_zig_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            let callee_name = node_text(func, source).to_string();
            // Skip builtin calls like @import, @sqrt
            if !callee_name.starts_with('@') {
                calls.push(HeuristicCall {
                    caller: caller.to_string(),
                    callee_name,
                    span: node_range(node),
                    confidence: 0.7,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls_recursive(child, source, caller, calls);
    }
}

/// Check if a `variable_declaration` is `const` (vs `var`).
fn is_const_decl(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("const ") || trimmed.starts_with("pub const ")
}

/// Check if a `variable_declaration` has a container type value.
fn has_type_value(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_declaration"
            | "enum_declaration"
            | "union_declaration"
            | "error_set_declaration"
            | "opaque_declaration" => return true,
            _ => {}
        }
    }
    false
}

/// Extract the import path from `const x = @import("path")`.
fn extract_import_path<'a>(node: tree_sitter::Node<'_>, source: &'a str) -> Option<&'a str> {
    let text = node_text(node, source);
    let at_idx = text.find("@import(")?;
    let start = at_idx + "@import(".len();
    let rest = &text[start..];
    // Skip opening quote
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

/// Extract test name from `test_declaration`.
fn extract_test_name(node: tree_sitter::Node<'_>, source: &str) -> Option<String> {
    // test_declaration children: "test" keyword, string or identifier, block
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            let text = node_text(child, source);
            return Some(text.trim_matches('"').to_string());
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source).to_string());
        }
    }
    None
}

/// Check if a declaration is public (has `pub` keyword prefix).
fn is_pub(node: tree_sitter::Node<'_>, source: &str) -> bool {
    let text = node_text(node, source);
    let trimmed = text.trim_start();
    trimmed.starts_with("pub ")
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
        "variable_declaration" => {
            scope_var_decl(node, source, scope, builder, exported);
        }
        "test_declaration" => {
            // Tests define a function but are never exported
            scope_test_decl(node, source, scope, builder);
        }
        "call_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported);
        }
        "container_field" => {
            scope_container_field(node, source, scope, builder);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported);
        }
    }
}

fn scope_func_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
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
    if is_pub(node, source) {
        exported.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));

    // Add parameters as definitions in the function scope
    if let Some(params) = find_child_by_kind(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }

    if let Some(body) = node.child_by_field_name("body") {
        scope_walk(body, source, func_scope, builder, exported);
    }
}

fn scope_var_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let Some(name_node) = find_child_by_kind(node, "identifier") else {
        return;
    };
    let name = node_text(name_node, source);

    let kind = if has_type_value(node) {
        SymbolKind::Type
    } else if is_const_decl(node_text(node, source)) {
        SymbolKind::Constant
    } else {
        SymbolKind::Variable
    };

    let def_id = builder.add_definition(scope, name, Some(node_range(name_node)), Some(kind));
    if is_pub(node, source) {
        exported.push(def_id);
    }

    // Check for @import — create import scope
    if let Some(import_path) = extract_import_path(node, source) {
        let import_scope = builder.add_import_scope();
        builder.add_import_reference(import_scope, import_path, Some(node_range(node)));
    }

    // If it's a container type (struct/enum/union), create child scope for members
    if has_type_value(node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "struct_declaration" | "enum_declaration" | "union_declaration"
                | "opaque_declaration" => {
                    let type_scope = builder.add_scope(scope, Some(node_range(child)));
                    // Walk inside the container to find fields/methods
                    scope_walk(child, source, type_scope, builder, exported);
                }
                "error_set_declaration" => {
                    let type_scope = builder.add_scope(scope, Some(node_range(child)));
                    scope_error_set(child, source, type_scope, builder);
                }
                _ => {}
            }
        }
    }
}

fn scope_test_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let name = extract_test_name(node, source).unwrap_or_else(|| "test".to_string());
    // Define the test but do NOT add to exported
    builder.add_definition(
        scope,
        &name,
        Some(node_range(node)),
        Some(SymbolKind::Function),
    );

    let test_scope = builder.add_scope(scope, Some(node_range(node)));
    // Walk body for references
    let mut dummy_exported = Vec::new();
    if let Some(body) = find_child_by_kind(node, "block") {
        scope_walk(body, source, test_scope, builder, &mut dummy_exported);
    }
}

fn scope_call(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(func) = node.child_by_field_name("function") else {
        return;
    };
    let target = match func.kind() {
        "identifier" => {
            let name = node_text(func, source);
            // Skip builtins like @import
            if name.starts_with('@') {
                None
            } else {
                Some(name.to_string())
            }
        }
        "field_expression" => {
            // obj.method — reference the method
            func.child_by_field_name("member")
                .map(|m| node_text(m, source).to_string())
        }
        _ => None,
    };
    if let Some(name) = target {
        builder.add_reference(
            scope,
            &name,
            Some(node_range(func)),
            Some(SymbolKind::Function),
        );
    }
}

fn scope_container_field(
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

fn scope_error_set(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = node_text(child, source);
            builder.add_definition(
                scope,
                name,
                Some(node_range(child)),
                Some(SymbolKind::Constant),
            );
        }
    }
}

fn scope_params(
    params_node: tree_sitter::Node<'_>,
    source: &str,
    func_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        if child.kind() == "parameter" {
            if let Some(name_node) = child.child_by_field_name("name") {
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

    fn parse_zig(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_zig::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    // ── Heuristic tests ──────────────────────────────────────────

    #[test]
    fn extracts_functions() {
        let source = "pub fn greet(name: []const u8) void {}\nfn helper() void {}\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("main.zig"))
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
                .any(|d| d.name == "helper" && d.kind == SymbolKind::Function),
            "Should find helper function, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_types() {
        let source = "const Point = struct { x: f64, y: f64 };\npub const Color = enum { red, green, blue };\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("types.zig"))
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
        let source = "const std = @import(\"std\");\nconst fs = @import(\"fs\");\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("main.zig"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should find at least 2 imports, got: {:?}",
            graph.imports
        );
        assert!(graph.imports.iter().any(|i| i.imported_name == "std"));
        assert!(graph.imports.iter().any(|i| i.imported_name == "fs"));
    }

    #[test]
    fn extracts_calls() {
        let source = "fn greet() void {}\npub fn main() void {\n    greet();\n}\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("main.zig"))
            .unwrap();

        assert!(
            !graph.calls.is_empty(),
            "Should extract call to greet, got: {:?}",
            graph.calls
        );
        assert!(
            graph.calls.iter().any(|c| c.callee_name == "greet"),
            "Should find call to greet"
        );
    }

    #[test]
    fn extracts_test_declarations() {
        let source = "test \"basic test\" {\n    const x = 1;\n}\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("test.zig"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "basic test" && d.kind == SymbolKind::Function),
            "Should find test declaration, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn doc_comment_extracted() {
        let source = "/// Does something important.\npub fn do_thing() void {}\n";
        let tree = parse_zig(source);
        let graph = ZigSupport
            .extract_heuristic(&tree, source, Path::new("lib.zig"))
            .unwrap();

        let def = graph
            .definitions
            .iter()
            .find(|d| d.name == "do_thing")
            .expect("should find do_thing");
        assert!(
            def.doc_comment.is_some(),
            "Should extract doc comment for do_thing"
        );
        assert!(
            def.doc_comment
                .as_ref()
                .unwrap()
                .text
                .contains("Does something important")
        );
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_zig(source);
        ZigSupport
            .build_scope_graph(&tree, source, Path::new("test.zig"))
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
        let sg = build_scope("pub fn greet(name: []const u8) void {}\nfn helper() void {}\n");
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
    fn scope_graph_exports() {
        let sg = build_scope("pub fn exported() void {}\nfn private() void {}\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "exported")
                })
            }),
            "pub fn should be exported"
        );
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "private")
                })
            }),
            "Non-pub fn should not be exported"
        );
    }

    #[test]
    fn scope_graph_nested_scopes() {
        let sg = build_scope(
            "pub const Point = struct {\n    x: f64,\n    y: f64,\n\n    pub fn distance(self: Point) f64 {\n        return 0;\n    }\n};\n",
        );
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Point"),
            "Should have type Point, got: {defs:?}"
        );
        assert!(defs.contains(&"x"), "Should have field x, got: {defs:?}");
        assert!(defs.contains(&"y"), "Should have field y, got: {defs:?}");
        assert!(
            defs.contains(&"distance"),
            "Should have method distance, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_imports() {
        let sg = build_scope("const std = @import(\"std\");\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"std"),
            "Should bind import std, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"std"),
            "Should reference import std, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_call_references() {
        let sg = build_scope("fn foo() void {}\nfn bar() void { foo(); }\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should reference foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "fn helper() void {}\nfn run() void { helper(); }\n";
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
    fn scope_graph_test_not_exported() {
        let sg = build_scope("pub fn greet() void {}\ntest \"basic\" {\n    greet();\n}\n");
        // The test should be defined but NOT exported
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"basic"), "Should define test, got: {defs:?}");
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "basic")
                })
            }),
            "Test should not be exported"
        );
    }

    #[test]
    fn scope_graph_enum_variants() {
        let sg = build_scope("pub const Color = enum { red, green, blue };\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Color"),
            "Should have enum Color, got: {defs:?}"
        );
        assert!(
            defs.contains(&"red"),
            "Should have variant red, got: {defs:?}"
        );
        assert!(
            defs.contains(&"green"),
            "Should have variant green, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_error_set() {
        let sg = build_scope("pub const MyError = error{ OutOfMemory, Overflow };\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"MyError"),
            "Should have error set MyError, got: {defs:?}"
        );
        assert!(
            defs.contains(&"OutOfMemory"),
            "Should have error OutOfMemory, got: {defs:?}"
        );
    }
}
