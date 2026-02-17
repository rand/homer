use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, extract_doc_comment_above, find_child_by_kind, node_range,
    node_text, qualified_name,
};

#[derive(Debug)]
pub struct RustSupport;

impl LanguageSupport for RustSupport {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn build_scope_graph(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<Option<FileScopeGraph>> {
        let mut builder = ScopeGraphBuilder::new(path);
        let root = builder.root();
        let mut pub_defs = Vec::new();

        scope_walk(tree.root_node(), source, root, &mut builder, &mut pub_defs, true);

        for def_id in &pub_defs {
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
        walk_rust_node(
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

fn walk_rust_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "function_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = qualified_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::Rustdoc, "///");

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                // Walk body for calls
                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_in_body(body, source, &qname, calls);
                }
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = qualified_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::Rustdoc, "///");

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: doc,
                });
            }
        }
        "impl_item" => {
            // Push impl target as context for methods
            if let Some(type_node) = child_by_field(node, "type") {
                let type_name = node_text(type_node, source).to_string();
                context.push(type_name);
                walk_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "mod_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = qualified_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Module,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::Rustdoc, "///"),
                });

                // Walk body with module context
                context.push(name);
                walk_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "const_item" | "static_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = qualified_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname,
                    kind: SymbolKind::Constant,
                    span: node_range(node),
                    doc_comment: extract_doc_comment_above(node, source, DocStyle::Rustdoc, "///"),
                });
            }
        }
        "use_declaration" => {
            extract_rust_use(node, source, path, imports);
        }
        _ => {}
    }

    walk_children(node, source, path, context, defs, calls, imports);
}

fn walk_children(
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
        walk_rust_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_in_body(
    body: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        extract_calls_recursive(child, source, caller, calls);
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
            let target = node_text(func, source).to_string();
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: target,
                span: node_range(node),
                confidence: 0.7,
            });
        }
    } else if node.kind() == "macro_invocation" {
        if let Some(mac) = child_by_field(node, "macro") {
            let target = format!("{}!", node_text(mac, source));
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: target,
                span: node_range(node),
                confidence: 0.6,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls_recursive(child, source, caller, calls);
    }
}

fn extract_rust_use(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    imports: &mut Vec<HeuristicImport>,
) {
    // Extract the use path text
    let use_text = node_text(node, source);
    // Strip "use " prefix and ";" suffix
    let import_path = use_text
        .strip_prefix("use ")
        .unwrap_or(use_text)
        .trim_end_matches(';')
        .trim();

    imports.push(HeuristicImport {
        from_path: path.to_path_buf(),
        imported_name: import_path.to_string(),
        target_path: None,
        confidence: 0.8,
    });
}

// ── Scope graph construction ─────────────────────────────────────────

use crate::scope_graph::ScopeNodeId;

fn scope_walk(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, pub_defs, is_module_level);
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    match node.kind() {
        "function_item" | "function_signature_item" => {
            scope_function(node, source, scope, builder, pub_defs, is_module_level);
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" => {
            scope_type_def(node, source, scope, builder, pub_defs, is_module_level);
        }
        "impl_item" => {
            scope_impl(node, source, scope, builder, pub_defs);
        }
        "mod_item" => {
            scope_mod(node, source, scope, builder, pub_defs, is_module_level);
        }
        "use_declaration" => {
            scope_use(node, source, scope, builder, pub_defs, is_module_level);
        }
        "const_item" | "static_item" => {
            scope_const(node, source, scope, builder, pub_defs, is_module_level);
        }
        "call_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, pub_defs, false);
        }
        _ => {
            scope_walk(node, source, scope, builder, pub_defs, is_module_level);
        }
    }
}

fn has_pub(node: tree_sitter::Node<'_>) -> bool {
    find_child_by_kind(node, "visibility_modifier").is_some()
}

fn scope_function(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Function),
    );
    if is_module_level && has_pub(node) {
        pub_defs.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, func_scope, builder, pub_defs, false);
    }
}

fn scope_type_def(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Type),
    );
    if is_module_level && has_pub(node) {
        pub_defs.push(def_id);
    }

    // For traits, create a child scope for associated items
    if node.kind() == "trait_item" {
        if let Some(body) = child_by_field(node, "body") {
            let trait_scope = builder.add_scope(scope, Some(node_range(node)));
            scope_walk(body, source, trait_scope, builder, pub_defs, false);
        }
    }
}

fn scope_impl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
) {
    // `impl Foo` or `impl Trait for Foo`
    // Reference the type being implemented
    if let Some(type_node) = child_by_field(node, "type") {
        let type_name = node_text(type_node, source);
        builder.add_reference(
            scope, type_name, Some(node_range(type_node)), Some(SymbolKind::Type),
        );
    }

    // Reference the trait if `impl Trait for Type`
    if let Some(trait_node) = child_by_field(node, "trait") {
        let trait_name = node_text(trait_node, source);
        builder.add_reference(
            scope, trait_name, Some(node_range(trait_node)), Some(SymbolKind::Type),
        );
    }

    // Methods in impl block get their own scope linked to parent
    let impl_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, impl_scope, builder, pub_defs, false);
    }
}

fn scope_mod(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Module),
    );
    if is_module_level && has_pub(node) {
        pub_defs.push(def_id);
    }

    // Inline mod body gets its own scope
    if let Some(body) = child_by_field(node, "body") {
        let mod_scope = builder.add_scope(scope, Some(node_range(body)));
        scope_walk(body, source, mod_scope, builder, pub_defs, true);
    }
}

fn scope_use(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let is_pub = has_pub(node);
    let import_scope = builder.add_import_scope();

    // Find the use argument (the path/tree after `use`)
    if let Some(arg) = child_by_field(node, "argument") {
        collect_use_bindings(arg, source, scope, import_scope, builder, pub_defs, is_module_level && is_pub);
    }
}

fn collect_use_bindings(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    import_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_pub: bool,
) {
    match node.kind() {
        "use_as_clause" => {
            // use foo::bar as baz
            let original = child_by_field(node, "path")
                .map(|n| use_leaf_name(n, source))
                .unwrap_or_default();
            let alias = child_by_field(node, "alias")
                .map_or_else(|| original.clone(), |n| node_text(n, source).to_string());

            if !original.is_empty() {
                builder.add_import_reference(import_scope, &original, Some(node_range(node)));
                let def_id = builder.add_definition(scope, &alias, Some(node_range(node)), None);
                if is_pub {
                    pub_defs.push(def_id);
                }
            }
        }
        "use_list" => {
            // use foo::{bar, baz}
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_use_bindings(child, source, scope, import_scope, builder, pub_defs, is_pub);
            }
        }
        "use_wildcard" => {
            // use foo::*
            builder.add_import_reference(import_scope, "*", Some(node_range(node)));
        }
        "scoped_use_list" => {
            // use foo::{bar, baz} — the scoped_use_list wraps the path + use_list
            if let Some(list) = find_child_by_kind(node, "use_list") {
                let mut cursor = list.walk();
                for child in list.children(&mut cursor) {
                    collect_use_bindings(child, source, scope, import_scope, builder, pub_defs, is_pub);
                }
            }
        }
        "scoped_identifier" | "identifier" => {
            // use foo::bar (simple path)
            let name = use_leaf_name(node, source);
            if !name.is_empty() {
                builder.add_import_reference(import_scope, &name, Some(node_range(node)));
                let def_id = builder.add_definition(scope, &name, Some(node_range(node)), None);
                if is_pub {
                    pub_defs.push(def_id);
                }
            }
        }
        _ => {}
    }
}

/// Extract the leaf name from a use path (last segment of `scoped_identifier`).
fn use_leaf_name(node: tree_sitter::Node<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source).to_string(),
        "scoped_identifier" => {
            child_by_field(node, "name")
                .map_or_else(String::new, |n| node_text(n, source).to_string())
        }
        _ => String::new(),
    }
}

fn scope_const(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    pub_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope, name, Some(node_range(name_node)), Some(SymbolKind::Constant),
    );
    if is_module_level && has_pub(node) {
        pub_defs.push(def_id);
    }
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
        "scoped_identifier" => {
            child_by_field(func, "name").map(|n| node_text(n, source).to_string())
        }
        "field_expression" => {
            child_by_field(func, "field").map(|n| node_text(n, source).to_string())
        }
        _ => None,
    };
    if let Some(name) = target {
        builder.add_reference(scope, &name, Some(node_range(func)), Some(SymbolKind::Function));
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
        match child.kind() {
            "parameter" => {
                if let Some(pattern) = child_by_field(child, "pattern") {
                    if pattern.kind() == "identifier" {
                        let name = node_text(pattern, source);
                        builder.add_definition(
                            func_scope, name, Some(node_range(pattern)), Some(SymbolKind::Variable),
                        );
                    }
                }
            }
            "self_parameter" => {
                builder.add_definition(
                    func_scope, "self", Some(node_range(child)), Some(SymbolKind::Variable),
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_rust(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_definitions() {
        let source = r"
/// Says hello.
fn hello() {}

fn add(a: i32, b: i32) -> i32 { a + b }
";
        let tree = parse_rust(source);
        let lang = RustSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.rs"))
            .unwrap();

        assert_eq!(graph.definitions.len(), 2);
        assert_eq!(graph.definitions[0].name, "hello");
        assert_eq!(graph.definitions[0].kind, SymbolKind::Function);
        assert!(graph.definitions[0].doc_comment.is_some());
        assert_eq!(graph.definitions[1].name, "add");
    }

    #[test]
    fn extracts_struct_and_enum() {
        let source = r"
/// A point.
struct Point { x: f64, y: f64 }

enum Color { Red, Green, Blue }
";
        let tree = parse_rust(source);
        let lang = RustSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.rs"))
            .unwrap();

        assert_eq!(graph.definitions.len(), 2);
        assert_eq!(graph.definitions[0].name, "Point");
        assert_eq!(graph.definitions[0].kind, SymbolKind::Type);
        assert!(graph.definitions[0].doc_comment.is_some());
        assert_eq!(graph.definitions[1].name, "Color");
    }

    #[test]
    fn extracts_calls() {
        let source = r#"
fn greet() {
    println!("hi");
    helper();
}

fn helper() {}
"#;
        let tree = parse_rust(source);
        let lang = RustSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.rs"))
            .unwrap();

        assert_eq!(graph.calls.len(), 2);
        assert_eq!(graph.calls[0].caller, "greet");
        assert_eq!(graph.calls[0].callee_name, "println!");
        assert_eq!(graph.calls[1].callee_name, "helper");
    }

    #[test]
    fn extracts_use_imports() {
        let source = r"
use std::collections::HashMap;
use crate::types::{Node, NodeKind};
";
        let tree = parse_rust(source);
        let lang = RustSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.rs"))
            .unwrap();

        assert_eq!(graph.imports.len(), 2);
        assert_eq!(graph.imports[0].imported_name, "std::collections::HashMap");
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_rust(source);
        RustSupport
            .build_scope_graph(&tree, source, Path::new("test.rs"))
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
        let sg = build_scope("pub fn greet(name: &str) {}\nfn helper() {}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(defs.contains(&"helper"), "Should have helper, got: {defs:?}");
        assert!(defs.contains(&"name"), "Should have param name, got: {defs:?}");
    }

    #[test]
    fn scope_graph_pub_exported() {
        let sg = build_scope("pub fn exported() {}\nfn private() {}\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "exported"))
            }),
            "pub fn exported should be exported"
        );
        assert!(
            !sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "private"))
            }),
            "fn private should not be exported"
        );
    }

    #[test]
    fn scope_graph_struct_and_enum() {
        let sg = build_scope("pub struct Foo;\nenum Bar { A, B }\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Foo"), "Should have Foo, got: {defs:?}");
        assert!(defs.contains(&"Bar"), "Should have Bar, got: {defs:?}");
    }

    #[test]
    fn scope_graph_trait_def() {
        let sg = build_scope("pub trait Greet {\n    fn hello(&self);\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Greet"), "Should have trait Greet, got: {defs:?}");
        assert!(defs.contains(&"hello"), "Should have method hello, got: {defs:?}");
    }

    #[test]
    fn scope_graph_impl_references() {
        let sg = build_scope("struct Foo;\ntrait Bar {}\nimpl Bar for Foo {}\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"Foo"), "impl should reference type Foo, got: {refs:?}");
        assert!(refs.contains(&"Bar"), "impl should reference trait Bar, got: {refs:?}");
    }

    #[test]
    fn scope_graph_use_simple() {
        let sg = build_scope("use std::collections::HashMap;\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"HashMap"), "use should bind HashMap, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"HashMap"), "use should reference HashMap, got: {refs:?}");
    }

    #[test]
    fn scope_graph_use_list() {
        let sg = build_scope("use std::collections::{HashMap, HashSet};\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"HashMap"), "Should bind HashMap, got: {defs:?}");
        assert!(defs.contains(&"HashSet"), "Should bind HashSet, got: {defs:?}");
    }

    #[test]
    fn scope_graph_use_alias() {
        let sg = build_scope("use std::collections::HashMap as Map;\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"Map"), "Should bind alias Map, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"HashMap"), "Should reference original HashMap, got: {refs:?}");
    }

    #[test]
    fn scope_graph_use_wildcard() {
        let sg = build_scope("use std::collections::*;\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"*"), "Wildcard should reference *, got: {refs:?}");
    }

    #[test]
    fn scope_graph_mod_inline() {
        let sg = build_scope("pub mod inner {\n    pub fn foo() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"inner"), "Should have mod inner, got: {defs:?}");
        assert!(defs.contains(&"foo"), "Should have fn foo inside mod, got: {defs:?}");
    }

    #[test]
    fn scope_graph_const_static() {
        let sg = build_scope("pub const MAX: u32 = 100;\nstatic COUNT: u32 = 0;\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"MAX"), "Should have const MAX, got: {defs:?}");
        assert!(defs.contains(&"COUNT"), "Should have static COUNT, got: {defs:?}");
    }

    #[test]
    fn scope_graph_call_reference() {
        let sg = build_scope("fn foo() {}\nfn bar() { foo(); }\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should reference foo(), got: {refs:?}");
    }

    #[test]
    fn scope_graph_method_call() {
        let sg = build_scope("fn run() { obj.process(); }\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"process"), "Should reference .process(), got: {refs:?}");
    }

    #[test]
    fn scope_graph_self_param() {
        let sg = build_scope("struct Foo;\nimpl Foo {\n    fn bar(&self) {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"self"), "Should have self param, got: {defs:?}");
        assert!(defs.contains(&"bar"), "Should have method bar, got: {defs:?}");
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "fn helper() {}\nfn run() { helper(); }\n";
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
        // File A uses a name imported from file B
        let source_a = "use crate::utils::greet;\nfn run() { greet(); }\n";
        let sg_a = {
            let tree = parse_rust(source_a);
            RustSupport.build_scope_graph(&tree, source_a, Path::new("a.rs")).unwrap().unwrap()
        };
        // File B exports a public function
        let source_b = "pub fn greet() {}\n";
        let sg_b = {
            let tree = parse_rust(source_b);
            RustSupport.build_scope_graph(&tree, source_b, Path::new("b.rs")).unwrap().unwrap()
        };

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg_a);
        scope_graph.add_file_graph(&sg_b);
        let resolved = scope_graph.resolve_all();

        let cross_file: Vec<_> = resolved.iter()
            .filter(|r| r.symbol == "greet" && r.definition_file == std::path::PathBuf::from("b.rs"))
            .collect();
        assert!(!cross_file.is_empty(), "use greet should resolve cross-file, got: {resolved:?}");
    }

    #[test]
    fn extracts_impl_methods() {
        let source = r"
struct Foo;

impl Foo {
    fn bar(&self) {}
    fn baz() {}
}
";
        let tree = parse_rust(source);
        let lang = RustSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.rs"))
            .unwrap();

        let fn_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .collect();
        assert_eq!(fn_defs.len(), 2);
        assert_eq!(fn_defs[0].qualified_name, "Foo::bar");
        assert_eq!(fn_defs[1].qualified_name, "Foo::baz");
    }
}
