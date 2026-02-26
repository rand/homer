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
pub struct PhpSupport;

impl LanguageSupport for PhpSupport {
    fn id(&self) -> &'static str {
        "php"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_php::LANGUAGE_PHP.into()
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
            true,
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

        walk_php_node(
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
fn walk_php_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "php_tag" => return,
        "function_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_block_doc_comment(node, source, DocStyle::PhpDoc);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                if let Some(body) = child_by_field(node, "body") {
                    heuristic_extract_calls(body, source, &qname, calls);
                }
            }
        }
        "class_declaration" | "interface_declaration" | "trait_declaration" => {
            heuristic_type_decl(node, source, path, context, defs, calls, imports);
            return;
        }
        "method_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_block_doc_comment(node, source, DocStyle::PhpDoc);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                if let Some(body) = child_by_field(node, "body") {
                    heuristic_extract_calls(body, source, &qname, calls);
                }
            }
        }
        "namespace_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let ns_name = node_text(name_node, source).to_string();
                context.push(ns_name);
                walk_php_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "namespace_use_declaration" => {
            heuristic_extract_use(node, source, path, imports);
        }
        "function_call_expression" => {
            if let Some(func) = child_by_field(node, "function") {
                let scope = if context.is_empty() {
                    "<global>".to_string()
                } else {
                    context.join(".")
                };
                calls.push(HeuristicCall {
                    caller: scope,
                    callee_name: node_text(func, source).to_string(),
                    span: node_range(node),
                    confidence: 0.7,
                });
            }
        }
        _ => {}
    }

    walk_php_children(node, source, path, context, defs, calls, imports);
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
    let doc = extract_block_doc_comment(node, source, DocStyle::PhpDoc);

    defs.push(HeuristicDef {
        name: name.clone(),
        qualified_name: qname,
        kind: SymbolKind::Type,
        span: node_range(node),
        doc_comment: doc,
    });

    context.push(name);
    walk_php_children(node, source, path, context, defs, calls, imports);
    context.pop();
}

fn walk_php_children(
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
        walk_php_node(child, source, path, context, defs, calls, imports);
    }
}

fn heuristic_extract_calls(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "function_call_expression" {
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
        heuristic_extract_calls(child, source, caller, calls);
    }
}

fn heuristic_extract_use(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    imports: &mut Vec<HeuristicImport>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "namespace_use_clause" {
            let text = node_text(child, source).to_string();
            imports.push(HeuristicImport {
                from_path: path.to_path_buf(),
                imported_name: text,
                target_path: None,
                confidence: 0.9,
            });
        }
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
    is_global: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, exported, is_global);
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_global: bool,
) {
    match node.kind() {
        "php_tag" => {}
        "function_definition" => {
            scope_function_def(node, source, scope, builder, exported, is_global);
        }
        "class_declaration" | "interface_declaration" | "trait_declaration" => {
            scope_class_like(node, source, scope, builder, exported, is_global);
        }
        "method_declaration" => {
            scope_method_decl(node, source, scope, builder);
        }
        "namespace_definition" => {
            scope_namespace(node, source, scope, builder, exported);
        }
        "namespace_use_declaration" => {
            scope_use_decl(node, source, scope, builder);
        }
        "function_call_expression" => {
            scope_call(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported, false);
        }
        _ => {
            scope_walk(node, source, scope, builder, exported, is_global);
        }
    }
}

fn scope_function_def(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_global: bool,
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
    if is_global {
        exported.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, func_scope, builder, exported, false);
    }
}

/// Handle class, interface, and trait declarations uniformly.
fn scope_class_like(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_global: bool,
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
    if is_global {
        exported.push(def_id);
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, class_scope, builder, exported, false);
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
        scope_walk(body, source, method_scope, builder, &mut ignored, false);
    }
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
        scope_walk(body, source, ns_scope, builder, exported, true);
    } else {
        // Namespace without braces: remaining siblings belong to namespace
        scope_walk(node, source, ns_scope, builder, exported, true);
    }
}

fn scope_use_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let import_scope = builder.add_import_scope();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "namespace_use_clause" {
            continue;
        }
        let symbol = extract_use_clause_name(child, source);
        if symbol.is_empty() {
            continue;
        }

        builder.add_import_reference(import_scope, &symbol, Some(node_range(child)));
        builder.add_definition(scope, &symbol, Some(node_range(child)), None);
    }
}

/// Extract the last name segment from a `namespace_use_clause`.
fn extract_use_clause_name(node: tree_sitter::Node<'_>, source: &str) -> String {
    // Try qualified_name first, then fallback to direct name child
    if let Some(qn) = find_child_by_kind(node, "qualified_name") {
        let mut cursor = qn.walk();
        if let Some(last) = qn
            .children(&mut cursor)
            .filter(|c| c.kind() == "name")
            .last()
        {
            return node_text(last, source).to_string();
        }
    }
    find_child_by_kind(node, "name")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_default()
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
        "name" => Some(node_text(func, source).to_string()),
        "qualified_name" => {
            let mut cursor = func.walk();
            func.children(&mut cursor)
                .filter(|c| c.kind() == "name")
                .last()
                .map(|n| node_text(n, source).to_string())
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

/// Extract parameter names from `formal_parameters`, stripping `$` prefix.
fn scope_params(
    params_node: tree_sitter::Node<'_>,
    source: &str,
    func_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        if child.kind() == "simple_parameter" {
            if let Some(var_name) = child_by_field(child, "name") {
                let name = extract_param_name(var_name, source);
                if !name.is_empty() {
                    builder.add_definition(
                        func_scope,
                        &name,
                        Some(node_range(child)),
                        Some(SymbolKind::Variable),
                    );
                }
            }
        }
    }
}

/// Extract the identifier part of a `variable_name` node, stripping `$`.
fn extract_param_name(var_name_node: tree_sitter::Node<'_>, source: &str) -> String {
    // variable_name contains a child `name` node (the identifier without $)
    if let Some(name_node) = find_child_by_kind(var_name_node, "name") {
        return node_text(name_node, source).to_string();
    }
    // Fallback: strip $ from the full text
    let text = node_text(var_name_node, source);
    text.strip_prefix('$').unwrap_or(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check whether a `method_declaration` node has `public` visibility.
    fn is_public_method(node: tree_sitter::Node<'_>, source: &str) -> bool {
        let mut cursor = node.walk();
        node.children(&mut cursor).any(|child| {
            child.kind() == "visibility_modifier" && node_text(child, source) == "public"
        })
    }

    /// Check whether a `method_declaration` node has `private` or `protected` visibility.
    fn is_private_or_protected(node: tree_sitter::Node<'_>, source: &str) -> bool {
        let mut cursor = node.walk();
        node.children(&mut cursor).any(|child| {
            child.kind() == "visibility_modifier" && {
                let vis = node_text(child, source);
                vis == "private" || vis == "protected"
            }
        })
    }

    fn parse_php(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_and_class() {
        let source = "<?php\nfunction greet() { echo 'hi'; }\n\nclass Greeter {\n    public function hello() {}\n}\n";
        let tree = parse_php(source);
        let graph = PhpSupport
            .extract_heuristic(&tree, source, Path::new("test.php"))
            .unwrap();

        let fn_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .collect();
        assert!(
            fn_defs.len() >= 2,
            "Should have at least 2 function defs, got: {fn_defs:?}"
        );
        assert_eq!(fn_defs[0].name, "greet");
        assert_eq!(fn_defs[1].qualified_name, "Greeter.hello");
    }

    #[test]
    fn extracts_interface_and_trait() {
        let source = "<?php\ninterface Loggable {\n    public function log();\n}\n\ntrait Cacheable {\n    public function cache() {}\n}\n";
        let tree = parse_php(source);
        let graph = PhpSupport
            .extract_heuristic(&tree, source, Path::new("test.php"))
            .unwrap();

        let type_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Type)
            .collect();
        assert!(
            type_defs.iter().any(|d| d.name == "Loggable"),
            "Should have interface Loggable, got: {type_defs:?}"
        );
        assert!(
            type_defs.iter().any(|d| d.name == "Cacheable"),
            "Should have trait Cacheable, got: {type_defs:?}"
        );
    }

    #[test]
    fn extracts_namespace_use() {
        let source = "<?php\nuse App\\Models\\User;\nuse App\\Services\\Auth;\n";
        let tree = parse_php(source);
        let graph = PhpSupport
            .extract_heuristic(&tree, source, Path::new("test.php"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should have at least 2 imports, got: {:?}",
            graph.imports
        );
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_php(source);
        PhpSupport
            .build_scope_graph(&tree, source, Path::new("test.php"))
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
        let sg = build_scope("<?php\nfunction greet() {}\nfunction helper() {}\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(
            defs.contains(&"helper"),
            "Should have helper, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_class_and_method() {
        let sg = build_scope("<?php\nclass Greeter {\n    public function hello() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Greeter"),
            "Should have class Greeter, got: {defs:?}"
        );
        assert!(
            defs.contains(&"hello"),
            "Should have method hello, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_interface() {
        let sg = build_scope("<?php\ninterface Loggable {\n    public function log();\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Loggable"),
            "Should have interface Loggable, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_trait() {
        let sg = build_scope("<?php\ntrait Cacheable {\n    public function cache() {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Cacheable"),
            "Should have trait Cacheable, got: {defs:?}"
        );
        assert!(
            defs.contains(&"cache"),
            "Should have method cache, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_namespace() {
        let sg = build_scope("<?php\nnamespace App\\Models {\n    class User {}\n}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.iter().any(|d| d.contains("App")),
            "Should have namespace def, got: {defs:?}"
        );
        assert!(
            defs.contains(&"User"),
            "Should have class User inside namespace, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_method_params() {
        let sg = build_scope("<?php\nfunction greet($name, $greeting) {}\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"name"),
            "Should have param name (without $), got: {defs:?}"
        );
        assert!(
            defs.contains(&"greeting"),
            "Should have param greeting (without $), got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_visibility() {
        // Test the visibility check helpers directly
        let source = "<?php\nclass Foo {\n    public function pubMethod() {}\n    private function privMethod() {}\n    protected function protMethod() {}\n}\n";
        let tree = parse_php(source);
        let root = tree.root_node();

        // Find method declarations
        let mut methods = Vec::new();
        collect_methods(root, &mut methods);

        let pub_method = methods.iter().find(|m| {
            child_by_field(**m, "name").is_some_and(|n| node_text(n, source) == "pubMethod")
        });
        let priv_method = methods.iter().find(|m| {
            child_by_field(**m, "name").is_some_and(|n| node_text(n, source) == "privMethod")
        });
        let prot_method = methods.iter().find(|m| {
            child_by_field(**m, "name").is_some_and(|n| node_text(n, source) == "protMethod")
        });

        assert!(pub_method.is_some(), "Should find pubMethod");
        assert!(is_public_method(*pub_method.unwrap(), source));
        assert!(!is_private_or_protected(*pub_method.unwrap(), source));

        assert!(priv_method.is_some(), "Should find privMethod");
        assert!(!is_public_method(*priv_method.unwrap(), source));
        assert!(is_private_or_protected(*priv_method.unwrap(), source));

        assert!(prot_method.is_some(), "Should find protMethod");
        assert!(!is_public_method(*prot_method.unwrap(), source));
        assert!(is_private_or_protected(*prot_method.unwrap(), source));
    }

    fn collect_methods<'a>(node: tree_sitter::Node<'a>, out: &mut Vec<tree_sitter::Node<'a>>) {
        if node.kind() == "method_declaration" {
            out.push(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_methods(child, out);
        }
    }

    #[test]
    fn scope_graph_namespace_use() {
        let sg = build_scope("<?php\nuse App\\Models\\User;\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"User"),
            "use should bind User, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"User"),
            "use should reference User, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_call_expression() {
        let sg = build_scope("<?php\nfunction foo() {}\nfunction bar() { foo(); }\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should reference foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "<?php\nfunction helper() {}\nfunction run() { helper(); }\n";
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
    fn scope_graph_global_function_exported() {
        let sg = build_scope("<?php\nfunction exported_fn() {}\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(
                            &n.kind,
                            ScopeNodeKind::PopSymbol { symbol } if symbol == "exported_fn"
                        )
                })
            }),
            "Global function should be exported"
        );
    }
}
