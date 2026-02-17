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
pub struct GoSupport;

impl LanguageSupport for GoSupport {
    fn id(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
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

        scope_walk(tree.root_node(), source, root, &mut builder, &mut exported_defs);

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

        walk_go_node(
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

fn walk_go_node(
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
                let doc = extract_doc_comment_above(node, source, DocStyle::Godoc, "//");

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
        "method_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                // Try to get receiver type for qualified name
                let receiver = child_by_field(node, "receiver")
                    .and_then(|r| find_child_by_kind(r, "type_identifier"))
                    .map(|t| node_text(t, source).to_string());

                let qname = if let Some(ref recv) = receiver {
                    format!("{recv}.{name}")
                } else {
                    dotted_name(context, &name)
                };

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::Godoc, "//"),
                });

                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "type_declaration" => {
            // Walk type specs inside
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_spec" {
                    if let Some(name_node) = child_by_field(child, "name") {
                        let name = node_text(name_node, source).to_string();
                        defs.push(HeuristicDef {
                            name: name.clone(),
                            qualified_name: dotted_name(context, &name),
                            kind: SymbolKind::Type,
                            span: node_range(child),
                            doc_comment: extract_doc_comment_above(
                                node,
                                source,
                                DocStyle::Godoc,
                                "//",
                            ),
                        });
                    }
                }
            }
        }
        "import_declaration" => {
            // Can be single or grouped
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "import_spec" || child.kind() == "interpreted_string_literal" {
                    let text = node_text(child, source).trim_matches('"').to_string();
                    imports.push(HeuristicImport {
                        from_path: path.to_path_buf(),
                        imported_name: text,
                        target_path: None,
                        confidence: 0.9,
                    });
                }
                if child.kind() == "import_spec_list" {
                    let mut inner = child.walk();
                    for spec in child.children(&mut inner) {
                        if spec.kind() == "import_spec" {
                            if let Some(path_node) = child_by_field(spec, "path") {
                                let text =
                                    node_text(path_node, source).trim_matches('"').to_string();
                                imports.push(HeuristicImport {
                                    from_path: path.to_path_buf(),
                                    imported_name: text,
                                    target_path: None,
                                    confidence: 0.9,
                                });
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_go_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "call_expression" {
        if let Some(func) = child_by_field(node, "function") {
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: node_text(func, source).to_string(),
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
        "method_declaration" => {
            scope_method_decl(node, source, scope, builder, exported);
        }
        "type_declaration" => {
            scope_type_decl(node, source, scope, builder, exported);
        }
        "import_declaration" => {
            scope_import(node, source, scope, builder);
        }
        "call_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported);
        }
        "short_var_declaration" | "var_declaration" => {
            scope_var_decl(node, source, scope, builder);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported);
        }
    }
}

/// In Go, identifiers starting with uppercase are exported.
fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
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
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Function),
    );
    if is_exported(name) {
        exported.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, func_scope, builder, exported);
    }
}

fn scope_method_decl(
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
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Function),
    );
    if is_exported(name) {
        exported.push(def_id);
    }

    // Reference the receiver type
    if let Some(receiver) = child_by_field(node, "receiver") {
        // receiver is a parameter_list containing parameter_declaration(s)
        // Look recursively for type_identifier within the receiver
        if let Some(type_node) = find_type_identifier_recursive(receiver) {
            let type_name = node_text(type_node, source);
            builder.add_reference(
                scope, type_name, Some(node_range(type_node)), Some(SymbolKind::Type),
            );
        }
    }

    let method_scope = builder.add_scope(scope, Some(node_range(node)));
    // Add receiver parameter name
    if let Some(receiver) = child_by_field(node, "receiver") {
        scope_params(receiver, source, method_scope, builder);
    }
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, method_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, method_scope, builder, exported);
    }
}

fn scope_type_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_spec" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                let def_id = builder.add_definition(
                    scope, name, Some(node_range(name_node)), Some(SymbolKind::Type),
                );
                if is_exported(name) {
                    exported.push(def_id);
                }

                // For struct/interface types, create a child scope for fields/methods
                if let Some(type_val) = child_by_field(child, "type") {
                    if type_val.kind() == "struct_type" || type_val.kind() == "interface_type" {
                        let type_scope = builder.add_scope(scope, Some(node_range(type_val)));
                        scope_struct_fields(type_val, source, type_scope, builder);
                    }
                }
            }
        }
    }
}

fn scope_struct_fields(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // Recursively find field/method declarations within the type body
    collect_type_members(node, source, scope, builder);
}

fn collect_type_members(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    match node.kind() {
        "field_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    scope, name, Some(node_range(name_node)), Some(SymbolKind::Field),
                );
            }
        }
        // Interface method specs — handle multiple possible node kinds
        "method_spec" | "method_elem" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    scope, name, Some(node_range(name_node)), Some(SymbolKind::Function),
                );
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_type_members(child, source, scope, builder);
            }
        }
    }
}

/// Recursively find the first `type_identifier` within a node tree.
fn find_type_identifier_recursive(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if node.kind() == "type_identifier" {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_type_identifier_recursive(child) {
            return Some(found);
        }
    }
    None
}

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_spec" => {
                scope_single_import(child, source, scope, builder);
            }
            "import_spec_list" => {
                let mut inner = child.walk();
                for spec in child.children(&mut inner) {
                    if spec.kind() == "import_spec" {
                        scope_single_import(spec, source, scope, builder);
                    }
                }
            }
            "interpreted_string_literal" => {
                // Single bare import: import "fmt"
                let pkg = import_pkg_name(node_text(child, source));
                if !pkg.is_empty() {
                    let import_scope = builder.add_import_scope();
                    builder.add_import_reference(import_scope, &pkg, Some(node_range(child)));
                    builder.add_definition(scope, &pkg, Some(node_range(child)), Some(SymbolKind::Module));
                }
            }
            _ => {}
        }
    }
}

fn scope_single_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let path_node = child_by_field(node, "path");
    let path_text = path_node.map_or("", |n| node_text(n, source));
    let pkg = import_pkg_name(path_text);

    if pkg.is_empty() {
        return;
    }

    // Check for alias: import alias "path"
    let alias = child_by_field(node, "name")
        .map(|n| node_text(n, source).to_string());

    let import_scope = builder.add_import_scope();
    builder.add_import_reference(import_scope, &pkg, Some(node_range(node)));

    let local_name = alias.as_deref().unwrap_or(&pkg);
    if local_name != "." && local_name != "_" {
        builder.add_definition(scope, local_name, Some(node_range(node)), Some(SymbolKind::Module));
    }
}

/// Extract the package name from a Go import path (last segment).
fn import_pkg_name(path: &str) -> String {
    let trimmed = path.trim_matches('"');
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

fn scope_call(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(func) = child_by_field(node, "function") else {
        return;
    };
    let target = match func.kind() {
        "identifier" => Some(node_text(func, source).to_string()),
        "selector_expression" => {
            // pkg.Func or obj.Method — reference the field/method
            child_by_field(func, "field").map(|f| node_text(f, source).to_string())
        }
        _ => None,
    };
    if let Some(name) = target {
        builder.add_reference(scope, &name, Some(node_range(func)), Some(SymbolKind::Function));
    }
}

fn scope_var_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // short_var_declaration: x := expr — "left" field contains the names
    if let Some(left) = child_by_field(node, "left") {
        let mut cursor = left.walk();
        for child in left.children(&mut cursor) {
            if child.kind() == "identifier" {
                let name = node_text(child, source);
                builder.add_definition(
                    scope, name, Some(node_range(child)), Some(SymbolKind::Variable),
                );
            }
        }
        return;
    }
    // var_declaration with var_spec children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "var_spec" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    scope, name, Some(node_range(name_node)), Some(SymbolKind::Variable),
                );
            }
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
        if child.kind() == "parameter_declaration" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    func_scope, name, Some(node_range(name_node)), Some(SymbolKind::Variable),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_go(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_and_type() {
        let source = "package main\n\n// Greet says hello.\nfunc Greet() {\n\tfmt.Println(\"hi\")\n}\n\ntype Point struct {\n\tX int\n\tY int\n}\n";
        let tree = parse_go(source);
        let graph = GoSupport
            .extract_heuristic(&tree, source, Path::new("main.go"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Greet" && d.kind == SymbolKind::Function)
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Point" && d.kind == SymbolKind::Type)
        );
        assert!(!graph.calls.is_empty());
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_go(source);
        GoSupport
            .build_scope_graph(&tree, source, Path::new("test.go"))
            .unwrap()
            .expect("should produce a scope graph")
    }

    fn pop_symbols(graph: &FileScopeGraph) -> Vec<&str> {
        graph.nodes.iter().filter_map(|n| match &n.kind {
            ScopeNodeKind::PopSymbol { symbol } => Some(symbol.as_str()),
            _ => None,
        }).collect()
    }

    fn push_symbols(graph: &FileScopeGraph) -> Vec<&str> {
        graph.nodes.iter().filter_map(|n| match &n.kind {
            ScopeNodeKind::PushSymbol { symbol } => Some(symbol.as_str()),
            _ => None,
        }).collect()
    }

    #[test]
    fn scope_graph_function() {
        let sg = build_scope("package main\n\nfunc Greet(name string) {}\nfunc helper() {}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Greet"), "Should have Greet, got: {defs:?}");
        assert!(defs.contains(&"helper"), "Should have helper, got: {defs:?}");
        assert!(defs.contains(&"name"), "Should have param name, got: {defs:?}");
    }

    #[test]
    fn scope_graph_exported_capitalized() {
        let sg = build_scope("package main\n\nfunc Exported() {}\nfunc private() {}\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "Exported"))
            }),
            "Capitalized func should be exported"
        );
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "private"))
            }),
            "Lowercase func should not be exported"
        );
    }

    #[test]
    fn scope_graph_type_struct() {
        let sg = build_scope("package main\n\ntype Point struct {\n\tX int\n\tY int\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Point"), "Should have type Point, got: {defs:?}");
        assert!(defs.contains(&"X"), "Should have field X, got: {defs:?}");
        assert!(defs.contains(&"Y"), "Should have field Y, got: {defs:?}");
    }

    #[test]
    fn scope_graph_type_interface() {
        let sg = build_scope("package main\n\ntype Reader interface {\n\tRead(p []byte) (int, error)\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Reader"), "Should have interface Reader, got: {defs:?}");
        assert!(defs.contains(&"Read"), "Should have method spec Read, got: {defs:?}");
    }

    #[test]
    fn scope_graph_method_with_receiver() {
        let sg = build_scope("package main\n\ntype Foo struct{}\n\nfunc (f *Foo) Bar() {}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Bar"), "Should have method Bar, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"Foo"), "Should reference receiver type Foo, got: {refs:?}");
    }

    #[test]
    fn scope_graph_import_single() {
        let sg = build_scope("package main\n\nimport \"fmt\"\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"fmt"), "Should bind import fmt, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"fmt"), "Should reference fmt, got: {refs:?}");
    }

    #[test]
    fn scope_graph_import_grouped() {
        let sg = build_scope("package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"fmt"), "Should bind fmt, got: {defs:?}");
        assert!(defs.contains(&"os"), "Should bind os, got: {defs:?}");
    }

    #[test]
    fn scope_graph_import_alias() {
        let sg = build_scope("package main\n\nimport f \"fmt\"\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"f"), "Should bind alias f, got: {defs:?}");
    }

    #[test]
    fn scope_graph_call_expression() {
        let sg = build_scope("package main\n\nfunc foo() {}\nfunc bar() { foo() }\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should reference foo(), got: {refs:?}");
    }

    #[test]
    fn scope_graph_selector_call() {
        let sg = build_scope("package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"hi\") }\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"Println"), "Should reference Println, got: {refs:?}");
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "package main\n\nfunc helper() {}\nfunc run() { helper() }\n";
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
    fn scope_graph_exported_defs_registered() {
        // In Go, same-package cross-file resolution happens implicitly (no imports).
        // The scope graph model handles this at the orchestration layer by merging
        // exports from all files in the same package. Verify exports are correctly
        // registered for both files.
        let source_a = "package main\n\nfunc Run() {}\n";
        let sg_a = {
            let tree = parse_go(source_a);
            GoSupport.build_scope_graph(&tree, source_a, Path::new("a.go")).unwrap().unwrap()
        };
        let source_b = "package main\n\nfunc Greet() {}\n";
        let sg_b = {
            let tree = parse_go(source_b);
            GoSupport.build_scope_graph(&tree, source_b, Path::new("b.go")).unwrap().unwrap()
        };

        // Both files should have their exported functions
        assert!(
            sg_a.export_nodes.iter().any(|&id| {
                sg_a.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "Run"))
            }),
            "a.go should export Run"
        );
        assert!(
            sg_b.export_nodes.iter().any(|&id| {
                sg_b.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "Greet"))
            }),
            "b.go should export Greet"
        );
    }

    #[test]
    fn extracts_imports() {
        let source = "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n";
        let tree = parse_go(source);
        let graph = GoSupport
            .extract_heuristic(&tree, source, Path::new("main.go"))
            .unwrap();
        assert!(graph.imports.len() >= 2);
    }
}
