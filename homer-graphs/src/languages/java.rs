use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, dotted_name, extract_block_doc_comment, find_child_by_kind,
    node_range, node_text,
};

#[derive(Debug)]
pub struct JavaSupport;

impl LanguageSupport for JavaSupport {
    fn id(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
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

        walk_java_node(
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

fn walk_java_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "class_declaration" | "interface_declaration" | "enum_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::Javadoc),
                });

                context.push(name);
                walk_java_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
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
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::Javadoc),
                });

                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "field_declaration" => {
            // Extract field names
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
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
        "import_declaration" => {
            let text = node_text(node, source);
            let import_path = text
                .strip_prefix("import ")
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

    walk_java_children(node, source, path, context, defs, calls, imports);
}

fn walk_java_children(
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
        walk_java_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "method_invocation" {
        if let Some(name_node) = child_by_field(node, "name") {
            let method_name = node_text(name_node, source);
            let target = if let Some(obj) = child_by_field(node, "object") {
                format!("{}.{method_name}", node_text(obj, source))
            } else {
                method_name.to_string()
            };
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: target,
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
    public_defs: &mut Vec<ScopeNodeId>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, public_defs);
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    public_defs: &mut Vec<ScopeNodeId>,
) {
    match node.kind() {
        "class_declaration" | "interface_declaration" | "enum_declaration" => {
            scope_type_decl(node, source, scope, builder, public_defs);
        }
        "method_declaration" | "constructor_declaration" => {
            scope_method_decl(node, source, scope, builder);
        }
        "field_declaration" => {
            scope_field_decl(node, source, scope, builder);
        }
        "import_declaration" => {
            scope_import(node, source, scope, builder);
        }
        "method_invocation" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, public_defs);
        }
        _ => {
            scope_walk(node, source, scope, builder, public_defs);
        }
    }
}

fn is_public(node: tree_sitter::Node<'_>) -> bool {
    let Some(mods) = find_child_by_kind(node, "modifiers") else {
        return false;
    };
    let mut cursor = mods.walk();
    mods.children(&mut cursor).any(|c| c.kind() == "public")
}

fn scope_type_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    public_defs: &mut Vec<ScopeNodeId>,
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

    if is_public(node) {
        public_defs.push(def_id);
    }

    // superclass reference: extends Foo
    // The "superclass" field wraps `extends <type>` — find the type inside
    if let Some(superclass) = child_by_field(node, "superclass") {
        collect_type_refs(superclass, source, scope, builder);
    }

    // interfaces: implements A, B (field name "interfaces" for class, also for interface extends)
    if let Some(interfaces) = child_by_field(node, "interfaces") {
        collect_type_refs(interfaces, source, scope, builder);
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, class_scope, builder, public_defs);
    }
}

/// Recursively find type references (`type_identifier`, `generic_type`, `scoped_type_identifier`)
/// within a wrapper node (`superclass`, `super_interfaces`, `type_list`, etc.) and add them as
/// `PushSymbol` references.
fn collect_type_refs(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    match node.kind() {
        "type_identifier" => {
            let name = node_text(node, source);
            builder.add_reference(scope, name, Some(node_range(node)), Some(SymbolKind::Type));
        }
        "generic_type" => {
            // e.g. List<String> — reference the outer type name
            if let Some(name_node) = find_child_by_kind(node, "type_identifier") {
                let name = node_text(name_node, source);
                builder.add_reference(
                    scope,
                    name,
                    Some(node_range(name_node)),
                    Some(SymbolKind::Type),
                );
            }
        }
        "scoped_type_identifier" => {
            // e.g. Map.Entry — reference the last segment
            let mut cursor = node.walk();
            if let Some(last) = node
                .children(&mut cursor)
                .filter(|c| c.kind() == "type_identifier")
                .last()
            {
                let name = node_text(last, source);
                builder.add_reference(scope, name, Some(node_range(last)), Some(SymbolKind::Type));
            }
        }
        _ => {
            // Recurse into children (superclass, super_interfaces, type_list wrappers)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_type_refs(child, source, scope, builder);
            }
        }
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

fn scope_field_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
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

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // Java imports: import java.util.List; or import java.util.*;
    let text = node_text(node, source);
    let import_path = text
        .strip_prefix("import ")
        .unwrap_or(text)
        .strip_prefix("static ")
        .unwrap_or(text)
        .trim_end_matches(';')
        .trim();

    if import_path.ends_with(".*") {
        // Wildcard import — create import scope but no specific symbol
        let import_scope = builder.add_import_scope();
        builder.add_import_reference(import_scope, "*", Some(node_range(node)));
    } else {
        // Named import: last segment is the symbol name
        let symbol = import_path.rsplit('.').next().unwrap_or(import_path);

        let import_scope = builder.add_import_scope();
        builder.add_import_reference(import_scope, symbol, Some(node_range(node)));
        // Bind the symbol in the current scope
        builder.add_definition(scope, symbol, Some(node_range(node)), None);
    }
}

fn scope_call(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    builder.add_reference(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Function),
    );
}

fn scope_params(
    params_node: tree_sitter::Node<'_>,
    source: &str,
    method_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
            if let Some(name_node) = child_by_field(child, "name") {
                let name = node_text(name_node, source);
                builder.add_definition(
                    method_scope,
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

    fn parse_java(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_class_and_method() {
        let source = "import java.util.List;\n\npublic class Foo {\n    public void bar() {\n        System.out.println(\"hi\");\n    }\n}\n";
        let tree = parse_java(source);
        let graph = JavaSupport
            .extract_heuristic(&tree, source, Path::new("Foo.java"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Foo" && d.kind == SymbolKind::Type)
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "bar" && d.qualified_name == "Foo.bar")
        );
        assert!(!graph.imports.is_empty());
        assert!(!graph.calls.is_empty());
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_java(source);
        JavaSupport
            .build_scope_graph(&tree, source, Path::new("Test.java"))
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
    fn scope_graph_class_and_method() {
        let sg = build_scope("public class Foo {\n    public void bar() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"bar"),
            "Should have method bar, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_interface() {
        let sg = build_scope("public interface Runnable {\n    void run();\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Runnable"),
            "Should have interface Runnable, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_enum() {
        let sg = build_scope("public enum Color { RED, GREEN, BLUE }\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Color"),
            "Should have enum Color, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_import_named() {
        let sg = build_scope("import java.util.List;\npublic class Foo {}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"List"),
            "Import should bind List, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"List"),
            "Import should reference List, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_import_wildcard() {
        let sg = build_scope("import java.util.*;\npublic class Foo {}\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"*"),
            "Wildcard import should reference *, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_method_params() {
        let sg = build_scope("public class Foo {\n    void bar(String name, int count) {}\n}\n");
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
    fn scope_graph_field_declaration() {
        let sg = build_scope("public class Foo {\n    private int x;\n    String name;\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"x"), "Should have field x, got: {defs:?}");
        assert!(
            defs.contains(&"name"),
            "Should have field name, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_extends_reference() {
        let sg = build_scope("class Foo extends Bar {}\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Bar"),
            "Should reference superclass Bar, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_implements_reference() {
        let sg = build_scope("class Foo implements Runnable, Serializable {}\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Runnable"),
            "Should ref Runnable, got: {refs:?}"
        );
        assert!(
            refs.contains(&"Serializable"),
            "Should ref Serializable, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_method_invocation() {
        let source = "public class Foo {\n    void bar() { baz(); }\n    void baz() {}\n}\n";
        let sg = build_scope(source);
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"baz"),
            "Should reference baz(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_constructor() {
        let sg = build_scope("public class Foo {\n    public Foo(String name) {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"name"),
            "Should have constructor param, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_public_exported() {
        let sg = build_scope("public class Foo {\n    public void bar() {}\n}\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "Foo")
                })
            }),
            "public class Foo should be exported"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "import java.util.List;\npublic class Foo {\n    void helper() {}\n    void run() { helper(); }\n}\n";
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
    fn scope_graph_cross_file_resolution() {
        // File A imports and uses a class from file B
        let source_a =
            "import com.example.Bar;\npublic class Foo {\n    void run() { greet(); }\n}\n";
        let sg_a = {
            let tree = parse_java(source_a);
            JavaSupport
                .build_scope_graph(&tree, source_a, Path::new("Foo.java"))
                .unwrap()
                .unwrap()
        };
        // File B exports a public class with a greet method
        let source_b = "public class Bar {\n    public void greet() {}\n}\n";
        let sg_b = {
            let tree = parse_java(source_b);
            JavaSupport
                .build_scope_graph(&tree, source_b, Path::new("Bar.java"))
                .unwrap()
                .unwrap()
        };

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg_a);
        scope_graph.add_file_graph(&sg_b);
        let resolved = scope_graph.resolve_all();

        let cross_file: Vec<_> = resolved
            .iter()
            .filter(|r| {
                r.symbol == "Bar" && r.definition_file == std::path::PathBuf::from("Bar.java")
            })
            .collect();
        assert!(
            !cross_file.is_empty(),
            "import Bar should resolve cross-file, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_inner_class() {
        let sg = build_scope(
            "public class Outer {\n    class Inner {\n        void work() {}\n    }\n}\n",
        );
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Outer"), "Should have Outer, got: {defs:?}");
        assert!(defs.contains(&"Inner"), "Should have Inner, got: {defs:?}");
        assert!(defs.contains(&"work"), "Should have work, got: {defs:?}");
    }
}
