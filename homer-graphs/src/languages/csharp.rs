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
pub struct CSharpSupport;

impl LanguageSupport for CSharpSupport {
    fn id(&self) -> &'static str {
        "csharp"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_c_sharp::LANGUAGE.into()
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

        walk_csharp_node(
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
fn walk_csharp_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "namespace_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Module,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::XmlDoc, "///"),
                });

                context.push(name);
                walk_csharp_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "class_declaration" | "struct_declaration" | "interface_declaration" => {
            heuristic_type_decl(node, source, path, context, defs, calls, imports);
            return;
        }
        "enum_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: dotted_name(context, &name),
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::XmlDoc, "///"),
                });
            }
        }
        "method_declaration" | "constructor_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::XmlDoc, "///"),
                });

                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "property_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: dotted_name(context, &name),
                    kind: SymbolKind::Field,
                    span: node_range(node),
                    doc_comment: None,
                });
            }
        }
        "field_declaration" => {
            heuristic_field_decl(node, source, context, defs);
        }
        "using_directive" => {
            let text = node_text(node, source);
            let import_path = text
                .strip_prefix("using ")
                .unwrap_or(text)
                .trim_end_matches(';')
                .trim()
                .to_string();
            imports.push(HeuristicImport {
                from_path: path.to_path_buf(),
                imported_name: import_path,
                target_path: None,
                confidence: 0.9,
            });
        }
        _ => {}
    }

    walk_csharp_children(node, source, path, context, defs, calls, imports);
}

fn heuristic_type_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source).to_string();
    let qname = dotted_name(context, &name);

    defs.push(HeuristicDef {
        name: name.clone(),
        qualified_name: qname,
        kind: SymbolKind::Type,
        span: node_range(node),
        doc_comment: extract_doc_comment_above(node, source, DocStyle::XmlDoc, "///"),
    });

    context.push(name);
    walk_csharp_children(node, source, path, context, defs, calls, imports);
    context.pop();
}

fn heuristic_field_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    context: &[String],
    defs: &mut Vec<HeuristicDef>,
) {
    // field_declaration → variable_declaration → variable_declarator → name
    if let Some(var_decl) = find_child_by_kind(node, "variable_declaration") {
        let mut cursor = var_decl.walk();
        for child in var_decl.children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                if let Some(name_node) = child_by_field(child, "name") {
                    let name = node_text(name_node, source).to_string();
                    defs.push(HeuristicDef {
                        name: name.clone(),
                        qualified_name: dotted_name(context, &name),
                        kind: SymbolKind::Field,
                        span: node_range(child),
                        doc_comment: None,
                    });
                }
            }
        }
    }
}

fn walk_csharp_children(
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
        walk_csharp_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "invocation_expression" {
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
        "namespace_declaration" => {
            scope_namespace(node, source, scope, builder, exported);
        }
        "class_declaration" | "struct_declaration" | "interface_declaration" => {
            scope_type_decl(node, source, scope, builder, exported);
        }
        "enum_declaration" => {
            scope_enum_decl(node, source, scope, builder, exported);
        }
        "method_declaration" => {
            scope_method_decl(node, source, scope, builder);
        }
        "constructor_declaration" => {
            scope_constructor_decl(node, source, scope, builder);
        }
        "property_declaration" => {
            scope_property_decl(node, source, scope, builder);
        }
        "field_declaration" => {
            scope_field_decl(node, source, scope, builder);
        }
        "using_directive" => {
            scope_using(node, source, scope, builder);
        }
        "invocation_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported);
        }
    }
}

/// Check whether a declaration has `public` or `internal` modifier children.
fn is_public_or_internal(node: tree_sitter::Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        if child.kind() == "modifier" {
            let text = node_text(child, source);
            matches!(text, "public" | "internal")
        } else {
            false
        }
    })
}

fn scope_namespace(
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
    builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Module),
    );

    let ns_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, ns_scope, builder, exported);
    }
}

fn scope_type_decl(
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

    if is_public_or_internal(node, source) {
        exported.push(def_id);
    }

    let type_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, type_scope, builder, exported);
    }
}

fn scope_enum_decl(
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

    if is_public_or_internal(node, source) {
        exported.push(def_id);
    }

    let enum_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, enum_scope, builder, exported);
    }
}

fn scope_method_decl(
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

    let method_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, method_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        let mut ignored = Vec::new();
        scope_walk(body, source, method_scope, builder, &mut ignored);
    }
}

fn scope_constructor_decl(
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

    let ctor_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, ctor_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        let mut ignored = Vec::new();
        scope_walk(body, source, ctor_scope, builder, &mut ignored);
    }
}

fn scope_property_decl(
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
        Some(SymbolKind::Field),
    );
}

fn scope_field_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // field_declaration → variable_declaration → variable_declarator → name
    let Some(var_decl) = find_child_by_kind(node, "variable_declaration") else {
        return;
    };
    let mut cursor = var_decl.walk();
    for child in var_decl.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    scope,
                    name,
                    Some(node_range(name_node)),
                    Some(SymbolKind::Field),
                );
            }
        }
    }
}

fn scope_using(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // Extract the last segment of the using path as the local binding name.
    let text = node_text(node, source);
    let import_path = text
        .strip_prefix("using ")
        .unwrap_or(text)
        .trim_end_matches(';')
        .trim();

    let symbol = import_path.rsplit('.').next().unwrap_or(import_path);
    if symbol.is_empty() {
        return;
    }

    let import_scope = builder.add_import_scope();
    builder.add_import_reference(import_scope, symbol, Some(node_range(node)));
    builder.add_definition(scope, symbol, Some(node_range(node)), None);
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
        "member_access_expression" => {
            // obj.Method() — reference the method name
            child_by_field(func, "name").map(|n| node_text(n, source).to_string())
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

fn scope_params(
    params_node: tree_sitter::Node<'_>,
    source: &str,
    func_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_csharp(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    // ── Heuristic tests ──────────────────────────────────────────────

    #[test]
    fn extracts_class_and_method() {
        let source = "using System;\n\npublic class Foo {\n    public void Bar() {\n        Console.WriteLine(\"hi\");\n    }\n}\n";
        let tree = parse_csharp(source);
        let graph = CSharpSupport
            .extract_heuristic(&tree, source, Path::new("Foo.cs"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Foo" && d.kind == SymbolKind::Type),
            "Should have class Foo, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Bar" && d.qualified_name == "Foo.Bar"),
            "Should have method Bar, got: {:?}",
            graph.definitions
        );
        assert!(!graph.imports.is_empty(), "Should have using directive");
        assert!(!graph.calls.is_empty(), "Should have calls");
    }

    #[test]
    fn extracts_namespace() {
        let source = "namespace MyApp.Core {\n    public class Service {\n        public void Run() {}\n    }\n}\n";
        let tree = parse_csharp(source);
        let graph = CSharpSupport
            .extract_heuristic(&tree, source, Path::new("Service.cs"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "MyApp.Core" && d.kind == SymbolKind::Module),
            "Should have namespace, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.qualified_name == "MyApp.Core.Service"),
            "Should have qualified class name, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_interface_and_enum() {
        let source = "public interface IService {\n    void Run();\n}\n\npublic enum Color { Red, Green, Blue }\n";
        let tree = parse_csharp(source);
        let graph = CSharpSupport
            .extract_heuristic(&tree, source, Path::new("Types.cs"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "IService" && d.kind == SymbolKind::Type),
            "Should have interface IService, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Color" && d.kind == SymbolKind::Type),
            "Should have enum Color, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_using_directives() {
        let source = "using System;\nusing System.Collections.Generic;\n\npublic class Foo {}\n";
        let tree = parse_csharp(source);
        let graph = CSharpSupport
            .extract_heuristic(&tree, source, Path::new("Foo.cs"))
            .unwrap();

        assert_eq!(graph.imports.len(), 2, "Should have 2 imports");
        assert_eq!(graph.imports[0].imported_name, "System");
        assert_eq!(graph.imports[1].imported_name, "System.Collections.Generic");
    }

    // ── Scope graph tests ────────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_csharp(source);
        CSharpSupport
            .build_scope_graph(&tree, source, Path::new("Test.cs"))
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
    fn scope_graph_namespace() {
        let sg = build_scope("namespace MyApp {\n    public class Foo {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"MyApp"),
            "Should have namespace MyApp, got: {defs:?}"
        );
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo inside namespace, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_class_and_method() {
        let sg = build_scope("public class Foo {\n    public void Bar() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"Bar"),
            "Should have method Bar, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_struct() {
        let sg = build_scope("public struct Point {\n    public int X;\n    public int Y;\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Point"),
            "Should have struct Point, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_interface() {
        let sg = build_scope("public interface IRunnable {\n    void Run();\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"IRunnable"),
            "Should have interface IRunnable, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_enum() {
        let sg = build_scope("public enum Color { Red, Green, Blue }\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Color"),
            "Should have enum Color, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_constructor() {
        let sg = build_scope("public class Foo {\n    public Foo(string name) {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class and constructor Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"name"),
            "Should have constructor param name, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_property() {
        let sg = build_scope("public class Foo {\n    public string Name { get; set; }\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Name"),
            "Should have property Name, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_method_params() {
        let sg =
            build_scope("public class Foo {\n    public void Bar(string name, int count) {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"name"),
            "Should have param name, got: {defs:?}"
        );
        assert!(
            defs.contains(&"count"),
            "Should have param count, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_visibility() {
        let sg =
            build_scope("public class Exported {}\nclass Private {}\ninternal class Internal {}\n");

        // Public class should be exported
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(
                            &n.kind,
                            ScopeNodeKind::PopSymbol { symbol } if symbol == "Exported"
                        )
                })
            }),
            "public class Exported should be exported"
        );

        // Private class should NOT be exported
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(
                            &n.kind,
                            ScopeNodeKind::PopSymbol { symbol } if symbol == "Private"
                        )
                })
            }),
            "class Private should not be exported"
        );

        // Internal class should be exported (visible within assembly)
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(
                            &n.kind,
                            ScopeNodeKind::PopSymbol { symbol } if symbol == "Internal"
                        )
                })
            }),
            "internal class Internal should be exported"
        );
    }

    #[test]
    fn scope_graph_using_directive() {
        let sg = build_scope("using System.Collections.Generic;\npublic class Foo {}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Generic"),
            "using should bind last segment Generic, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Generic"),
            "using should reference Generic, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "public class Foo {\n    void Helper() {}\n    void Run() { Helper(); }\n}\n";
        let sg = build_scope(source);
        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();
        assert!(
            resolved.iter().any(|r| r.symbol == "Helper"),
            "Helper() should resolve, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_field_declaration() {
        let sg = build_scope(
            "public class Foo {\n    private int _count;\n    private string _name;\n}\n",
        );
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"_count"),
            "Should have field _count, got: {defs:?}"
        );
        assert!(
            defs.contains(&"_name"),
            "Should have field _name, got: {defs:?}"
        );
    }
}
