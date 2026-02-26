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
pub struct KotlinSupport;

impl LanguageSupport for KotlinSupport {
    fn id(&self) -> &'static str {
        "kotlin"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["kt", "kts"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_kotlin_ng::LANGUAGE.into()
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

        walk_kotlin_node(
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
fn walk_kotlin_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "class_declaration" | "object_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::KDoc),
                });

                context.push(name);
                walk_kotlin_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "function_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::KDoc),
                });

                if let Some(body) = find_child_by_kind(node, "function_body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "import" => {
            extract_kotlin_import(node, source, path, imports);
        }
        _ => {}
    }

    walk_kotlin_children(node, source, path, context, defs, calls, imports);
}

fn walk_kotlin_children(
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
        walk_kotlin_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_kotlin_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    imports: &mut Vec<HeuristicImport>,
) {
    // The `import` node contains `qualified_identifier` as a child
    let text = node_text(node, source);
    let import_path = text
        .strip_prefix("import ")
        .unwrap_or(text)
        .trim()
        .to_string();

    imports.push(HeuristicImport {
        from_path: path.to_path_buf(),
        imported_name: import_path,
        target_path: None,
        confidence: 0.9,
    });
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "call_expression" {
        // call_expression has a function child (the thing being called)
        // and value_arguments. The function part can be an identifier,
        // navigation_expression, etc.
        let func_node = node.child(0);
        if let Some(func) = func_node {
            let target = node_text(func, source).to_string();
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

/// Check if a declaration has `private` or `internal` visibility modifiers.
fn is_private_or_internal(node: tree_sitter::Node<'_>) -> bool {
    let Some(mods) = find_child_by_kind(node, "modifiers") else {
        return false;
    };
    let mut cursor = mods.walk();
    mods.children(&mut cursor).any(|c| {
        c.kind() == "visibility_modifier" && {
            let mut inner = c.walk();
            c.children(&mut inner)
                .any(|v| v.kind() == "private" || v.kind() == "internal")
        }
    })
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
        "class_declaration" => {
            scope_class_decl(node, source, scope, builder, exported);
        }
        "object_declaration" => {
            scope_object_decl(node, source, scope, builder, exported);
        }
        "companion_object" => {
            scope_companion_object(node, source, scope, builder, exported);
        }
        "function_declaration" => {
            scope_func_decl(node, source, scope, builder, exported);
        }
        "import" => {
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

    if !is_private_or_internal(node) {
        exported.push(def_id);
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = find_child_by_kind(node, "class_body") {
        scope_walk(body, source, class_scope, builder, exported);
    }
}

fn scope_object_decl(
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

    if !is_private_or_internal(node) {
        exported.push(def_id);
    }

    let obj_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = find_child_by_kind(node, "class_body") {
        scope_walk(body, source, obj_scope, builder, exported);
    }
}

fn scope_companion_object(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
) {
    let companion_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = find_child_by_kind(node, "class_body") {
        scope_walk(body, source, companion_scope, builder, exported);
    }
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

    // Top-level functions: exported unless private/internal
    // We mark exported here; the caller only adds to exported_defs for
    // top-level declarations. Since scope_dispatch is called recursively,
    // nested functions will also get added — but that matches the pattern
    // of other language implementations (Java, Go) where nested defs are
    // also considered.
    if !is_private_or_internal(node) {
        exported.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    scope_params(node, source, func_scope, builder);
    if let Some(body) = find_child_by_kind(node, "function_body") {
        scope_walk(body, source, func_scope, builder, exported);
    }
}

fn scope_params(
    func_node: tree_sitter::Node<'_>,
    source: &str,
    func_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(params) = find_child_by_kind(func_node, "function_value_parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for child in params.children(&mut cursor) {
        if child.kind() == "parameter" {
            // In Kotlin's tree-sitter grammar, parameter has an `identifier`
            // child (not a named "name" field) followed by `:` and type.
            if let Some(name_node) = find_child_by_kind(child, "identifier") {
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

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let text = node_text(node, source);
    let import_path = text.strip_prefix("import ").unwrap_or(text).trim();

    if import_path.ends_with(".*") {
        let import_scope = builder.add_import_scope();
        builder.add_import_reference(import_scope, "*", Some(node_range(node)));
    } else {
        // Last segment is the symbol name
        let symbol = import_path.rsplit('.').next().unwrap_or(import_path);
        let import_scope = builder.add_import_scope();
        builder.add_import_reference(import_scope, symbol, Some(node_range(node)));
        builder.add_definition(scope, symbol, Some(node_range(node)), None);
    }
}

fn scope_call(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    // call_expression's first child is the function being called
    let Some(func) = node.child(0) else {
        return;
    };
    match func.kind() {
        "identifier" | "simple_identifier" => {
            let name = node_text(func, source);
            builder.add_reference(
                scope,
                name,
                Some(node_range(func)),
                Some(SymbolKind::Function),
            );
        }
        "navigation_expression" => {
            // obj.method — reference the last identifier (the method name)
            let mut cursor = func.walk();
            if let Some(last_ident) = func
                .children(&mut cursor)
                .filter(|c| c.kind() == "identifier" || c.kind() == "simple_identifier")
                .last()
            {
                let name = node_text(last_ident, source);
                builder.add_reference(
                    scope,
                    name,
                    Some(node_range(last_ident)),
                    Some(SymbolKind::Function),
                );
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_kotlin(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    // ── Heuristic extraction tests ───────────────────────────────────

    #[test]
    fn extracts_class_and_function() {
        let source = "package com.example\n\nclass Foo {\n    fun bar() {\n        println(\"hi\")\n    }\n}\n";
        let tree = parse_kotlin(source);
        let graph = KotlinSupport
            .extract_heuristic(&tree, source, Path::new("Foo.kt"))
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
                .any(|d| d.name == "bar" && d.qualified_name == "Foo.bar"),
            "Should have method bar, got: {:?}",
            graph.definitions
        );
        assert!(
            !graph.calls.is_empty(),
            "Should have calls, got: {:?}",
            graph.calls
        );
    }

    #[test]
    fn extracts_object_declaration() {
        let source = "object Singleton {\n    fun doWork() {}\n}\n";
        let tree = parse_kotlin(source);
        let graph = KotlinSupport
            .extract_heuristic(&tree, source, Path::new("Singleton.kt"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Singleton" && d.kind == SymbolKind::Type),
            "Should have object Singleton, got: {:?}",
            graph.definitions
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "doWork" && d.kind == SymbolKind::Function),
            "Should have function doWork, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_interface() {
        let source = "interface Greeter {\n    fun greet()\n}\n";
        let tree = parse_kotlin(source);
        let graph = KotlinSupport
            .extract_heuristic(&tree, source, Path::new("Greeter.kt"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Greeter" && d.kind == SymbolKind::Type),
            "Should have interface Greeter, got: {:?}",
            graph.definitions
        );
    }

    #[test]
    fn extracts_imports() {
        let source = "import com.example.Foo\nimport com.example.bar.Baz\n\nfun main() {}\n";
        let tree = parse_kotlin(source);
        let graph = KotlinSupport
            .extract_heuristic(&tree, source, Path::new("main.kt"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should have at least 2 imports, got: {:?}",
            graph.imports
        );
        assert!(
            graph
                .imports
                .iter()
                .any(|i| i.imported_name == "com.example.Foo"),
            "Should import com.example.Foo, got: {:?}",
            graph.imports
        );
    }

    // ── Scope graph tests ────────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_kotlin(source);
        KotlinSupport
            .build_scope_graph(&tree, source, Path::new("Test.kt"))
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
        let sg = build_scope("fun greet(name: String) {}\nfun helper() {}\n");
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
    fn scope_graph_class() {
        let sg = build_scope("class Foo {\n    fun bar() {}\n}\n");
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
        let sg = build_scope("interface Greeter {\n    fun greet()\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Greeter"),
            "Should have interface Greeter, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_object() {
        let sg = build_scope("object Singleton {\n    fun doWork() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Singleton"),
            "Should have object Singleton, got: {defs:?}"
        );
        assert!(
            defs.contains(&"doWork"),
            "Should have doWork, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_companion_object() {
        let sg = build_scope(
            "class Foo {\n    companion object {\n        fun create(): Foo = Foo()\n    }\n}\n",
        );
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
        assert!(
            defs.contains(&"create"),
            "Should have companion fun create, got: {defs:?}"
        );

        // Verify there is a scope node for the companion object (distinct from
        // the class body scope). We expect at least: root, class scope,
        // companion scope, function scope.
        let scope_count = sg
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, ScopeNodeKind::Scope))
            .count();
        assert!(
            scope_count >= 3,
            "Should have at least 3 scope nodes (class + companion + func), got: {scope_count}"
        );
    }

    #[test]
    fn scope_graph_visibility_modifiers() {
        let sg = build_scope(
            "fun publicFun() {}\nprivate fun privateFun() {}\ninternal fun internalFun() {}\n",
        );
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"publicFun"),
            "Should have publicFun, got: {defs:?}"
        );
        assert!(
            defs.contains(&"privateFun"),
            "Should have privateFun def, got: {defs:?}"
        );

        // publicFun should be exported
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "publicFun")
                })
            }),
            "publicFun should be exported"
        );
        // privateFun should NOT be exported
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "privateFun")
                })
            }),
            "privateFun should not be exported"
        );
        // internalFun should NOT be exported
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "internalFun")
                })
            }),
            "internalFun should not be exported"
        );
    }

    #[test]
    fn scope_graph_import() {
        let sg = build_scope("import com.example.Foo\n\nfun main() {}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Foo"),
            "Import should bind Foo, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Foo"),
            "Import should reference Foo, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_call_expression() {
        let sg = build_scope("fun foo() {}\nfun bar() { foo() }\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should reference foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "import com.example.List\n\nfun helper() {}\nfun run() { helper() }\n";
        let sg = build_scope(source);
        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();
        assert!(
            resolved.iter().any(|r| r.symbol == "helper"),
            "helper() should resolve, got: {resolved:?}"
        );
    }
}
