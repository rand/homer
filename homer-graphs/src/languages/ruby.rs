use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, dotted_name, extract_doc_comment_above, node_range,
    node_text,
};

#[derive(Debug)]
pub struct RubySupport;

impl LanguageSupport for RubySupport {
    fn id(&self) -> &'static str {
        "ruby"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rb"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_ruby::LANGUAGE.into()
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

        walk_ruby_node(
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
fn walk_ruby_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "module" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::Yard, "# ");

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Module,
                    span: node_range(node),
                    doc_comment: doc,
                });

                context.push(name);
                walk_ruby_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "class" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::Yard, "# ");

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: doc,
                });

                context.push(name);
                walk_ruby_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "method" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_doc_comment_above(node, source, DocStyle::Yard, "# ");

                defs.push(HeuristicDef {
                    name: name.clone(),
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
        "singleton_method" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &format!("self.{name}"));
                let doc = extract_doc_comment_above(node, source, DocStyle::Yard, "# ");

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
        "call" => {
            extract_require_import(node, source, path, imports);
            // Also record general calls at heuristic level
            if let Some(method_node) = child_by_field(node, "method") {
                let method_name = node_text(method_node, source);
                if method_name != "require" && method_name != "require_relative" {
                    let caller = if context.is_empty() {
                        "<module>".to_string()
                    } else {
                        context.join(".")
                    };
                    calls.push(HeuristicCall {
                        caller,
                        callee_name: method_name.to_string(),
                        span: node_range(node),
                        confidence: 0.7,
                    });
                }
            }
        }
        _ => {}
    }

    walk_ruby_children(node, source, path, context, defs, calls, imports);
}

fn walk_ruby_children(
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
        walk_ruby_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    match node.kind() {
        "call" => {
            if let Some(method_node) = child_by_field(node, "method") {
                let method_name = node_text(method_node, source);
                if method_name != "require" && method_name != "require_relative" {
                    calls.push(HeuristicCall {
                        caller: caller.to_string(),
                        callee_name: method_name.to_string(),
                        span: node_range(node),
                        confidence: 0.7,
                    });
                }
            }
        }
        // Bare identifier in a body_statement is a potential method call in Ruby
        "identifier" if node.parent().is_some_and(|p| p.kind() == "body_statement") => {
            let name = node_text(node, source);
            calls.push(HeuristicCall {
                caller: caller.to_string(),
                callee_name: name.to_string(),
                span: node_range(node),
                confidence: 0.5,
            });
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls_recursive(child, source, caller, calls);
    }
}

/// Extract `require` / `require_relative` calls as imports.
fn extract_require_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    imports: &mut Vec<HeuristicImport>,
) {
    let Some(method_node) = child_by_field(node, "method") else {
        return;
    };
    let method_name = node_text(method_node, source);
    if method_name != "require" && method_name != "require_relative" {
        return;
    }

    let Some(args) = child_by_field(node, "arguments") else {
        return;
    };
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "string" || child.kind() == "string_content" {
            let raw = node_text(child, source);
            let import_name = raw.trim_matches('"').trim_matches('\'').to_string();
            if !import_name.is_empty() {
                imports.push(HeuristicImport {
                    from_path: path.to_path_buf(),
                    imported_name: import_name,
                    target_path: None,
                    confidence: 0.9,
                });
            }
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
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scope_dispatch(child, source, scope, builder, exported, is_module_level);
    }
}

fn scope_dispatch(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    match node.kind() {
        "module" => {
            scope_module_decl(node, source, scope, builder, exported, is_module_level);
        }
        "class" => {
            scope_class_decl(node, source, scope, builder, exported, is_module_level);
        }
        "method" => {
            scope_method_decl(node, source, scope, builder, exported, is_module_level);
        }
        "singleton_method" => {
            scope_singleton_method(node, source, scope, builder, exported, is_module_level);
        }
        "call" => {
            scope_call_node(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported, false);
        }
        "assignment" => {
            scope_assignment(node, source, scope, builder);
            scope_walk(node, source, scope, builder, exported, false);
        }
        // Bare identifier in a body statement is a potential method call in Ruby
        // (e.g. `foo` without parens). Only treat as a reference if not at module
        // level, since module-level identifiers are also potential calls.
        "identifier"
            if node
                .parent()
                .is_some_and(|p| p.kind() == "body_statement" || p.kind() == "program") =>
        {
            let name = node_text(node, source);
            builder.add_reference(
                scope,
                name,
                Some(node_range(node)),
                Some(SymbolKind::Function),
            );
        }
        _ => {
            scope_walk(node, source, scope, builder, exported, is_module_level);
        }
    }
}

fn scope_module_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    let def_id = builder.add_definition(
        scope,
        name,
        Some(node_range(name_node)),
        Some(SymbolKind::Module),
    );
    if is_module_level {
        exported.push(def_id);
    }

    let mod_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, mod_scope, builder, exported, true);
    }
}

fn scope_class_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
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
    if is_module_level {
        exported.push(def_id);
    }

    // Reference superclass if present
    if let Some(superclass) = child_by_field(node, "superclass") {
        let sc_name = node_text(superclass, source);
        builder.add_reference(
            scope,
            sc_name,
            Some(node_range(superclass)),
            Some(SymbolKind::Type),
        );
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        // Class-level defs are exported (public by default in Ruby)
        scope_walk(body, source, class_scope, builder, exported, true);
    }
}

fn scope_method_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
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
    if is_module_level {
        exported.push(def_id);
    }

    let method_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, method_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, method_scope, builder, exported, false);
    }
}

fn scope_singleton_method(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    exported: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
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
    if is_module_level {
        exported.push(def_id);
    }

    let method_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, method_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        scope_walk(body, source, method_scope, builder, exported, false);
    }
}

fn scope_call_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(method_node) = child_by_field(node, "method") else {
        return;
    };
    let method_name = node_text(method_node, source);

    // Handle require/require_relative as imports
    if method_name == "require" || method_name == "require_relative" {
        scope_require_import(node, source, scope, builder);
        return;
    }

    // General call: add a reference for the method name
    builder.add_reference(
        scope,
        method_name,
        Some(node_range(method_node)),
        Some(SymbolKind::Function),
    );
}

fn scope_require_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(args) = child_by_field(node, "arguments") else {
        return;
    };
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "string" || child.kind() == "string_content" {
            let raw = node_text(child, source);
            let import_name = raw.trim_matches('"').trim_matches('\'');
            if !import_name.is_empty() {
                // Extract the last path segment as the local module name
                let local_name = import_name.rsplit('/').next().unwrap_or(import_name);
                let import_scope = builder.add_import_scope();
                builder.add_import_reference(import_scope, local_name, Some(node_range(child)));
                builder.add_definition(
                    scope,
                    local_name,
                    Some(node_range(child)),
                    Some(SymbolKind::Module),
                );
            }
        }
    }
}

fn scope_assignment(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(left) = child_by_field(node, "left") else {
        return;
    };
    if left.kind() == "identifier" {
        let name = node_text(left, source);
        builder.add_definition(
            scope,
            name,
            Some(node_range(left)),
            Some(SymbolKind::Variable),
        );
    }
}

fn scope_params(
    params_node: tree_sitter::Node<'_>,
    source: &str,
    method_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(child, source);
                builder.add_definition(
                    method_scope,
                    name,
                    Some(node_range(child)),
                    Some(SymbolKind::Variable),
                );
            }
            "optional_parameter"
            | "keyword_parameter"
            | "splat_parameter"
            | "hash_splat_parameter"
            | "block_parameter" => {
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
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ruby(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_method_and_class() {
        let source = "class Greeter\n  # Says hello.\n  def greet\n    puts \"hi\"\n  end\nend\n";
        let tree = parse_ruby(source);
        let graph = RubySupport
            .extract_heuristic(&tree, source, Path::new("test.rb"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.name == "Greeter" && d.kind == SymbolKind::Type)
        );
        assert!(graph.definitions.iter().any(|d| d.name == "greet"
            && d.kind == SymbolKind::Function
            && d.qualified_name == "Greeter.greet"));
    }

    #[test]
    fn extracts_module_nesting() {
        let source = "module Outer\n  class Inner\n    def work\n    end\n  end\nend\n";
        let tree = parse_ruby(source);
        let graph = RubySupport
            .extract_heuristic(&tree, source, Path::new("test.rb"))
            .unwrap();

        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.qualified_name == "Outer")
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.qualified_name == "Outer.Inner")
        );
        assert!(
            graph
                .definitions
                .iter()
                .any(|d| d.qualified_name == "Outer.Inner.work")
        );
    }

    #[test]
    fn extracts_require_imports() {
        let source = "require \"json\"\nrequire_relative \"helpers/util\"\n";
        let tree = parse_ruby(source);
        let graph = RubySupport
            .extract_heuristic(&tree, source, Path::new("test.rb"))
            .unwrap();

        assert!(
            graph.imports.len() >= 2,
            "Should have at least 2 imports, got: {:?}",
            graph.imports
        );
        assert!(
            graph.imports.iter().any(|i| i.imported_name == "json"),
            "Should import json, got: {:?}",
            graph.imports
        );
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_ruby(source);
        RubySupport
            .build_scope_graph(&tree, source, Path::new("test.rb"))
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
    fn scope_graph_method() {
        let sg = build_scope("def greet\nend\ndef helper\nend\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(
            defs.contains(&"helper"),
            "Should have helper, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_class_and_module() {
        let sg = build_scope("module MyMod\n  class MyClass\n    def foo\n    end\n  end\nend\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"MyMod"),
            "Should have module MyMod, got: {defs:?}"
        );
        assert!(
            defs.contains(&"MyClass"),
            "Should have class MyClass, got: {defs:?}"
        );
        assert!(
            defs.contains(&"foo"),
            "Should have method foo, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_singleton_method() {
        let sg = build_scope("class Foo\n  def self.bar\n  end\nend\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"bar"),
            "Should have singleton method bar, got: {defs:?}"
        );
        assert!(
            defs.contains(&"Foo"),
            "Should have class Foo, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_method_params() {
        let sg = build_scope("def greet(name, greeting = \"hi\")\nend\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"name"),
            "Should have param name, got: {defs:?}"
        );
        assert!(
            defs.contains(&"greeting"),
            "Should have param greeting, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_exported_defs() {
        let sg = build_scope("def greet\nend\nclass Foo\nend\n");
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "greet")
                })
            }),
            "Module-level method greet should be exported"
        );
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| {
                    n.id == id
                        && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "Foo")
                })
            }),
            "Module-level class Foo should be exported"
        );
    }

    #[test]
    fn scope_graph_require_import() {
        let sg = build_scope("require \"json\"\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"json"),
            "require should bind json, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"json"),
            "require should reference json, got: {refs:?}"
        );
        let imports: Vec<_> = sg
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, ScopeNodeKind::ImportScope))
            .collect();
        assert!(!imports.is_empty(), "Should have ImportScope node");
    }

    #[test]
    fn scope_graph_call_expression() {
        let sg = build_scope("def foo\nend\ndef bar\n  foo\nend\n");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should reference foo, got: {refs:?}");
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "def helper\nend\ndef run\n  helper\nend\n";
        let sg = build_scope(source);
        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();
        assert!(
            resolved.iter().any(|r| r.symbol == "helper"),
            "helper should resolve, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_nested_scope() {
        // Variables defined inside a method scope should not leak to outer scope
        let source = "def outer\n  x = 1\nend\ndef inner\n  y = 2\nend\n";
        let sg = build_scope(source);

        // Both methods and their local variables should exist as defs
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"outer"), "Should have outer, got: {defs:?}");
        assert!(defs.contains(&"inner"), "Should have inner, got: {defs:?}");
        assert!(defs.contains(&"x"), "Should have x, got: {defs:?}");
        assert!(defs.contains(&"y"), "Should have y, got: {defs:?}");

        // Verify that x and y are in separate scopes (scope graph structure)
        // by checking that a reference to x in inner's scope would NOT resolve
        // (x is defined in outer's scope, not inner's)
        let x_node = sg
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "x"))
            .expect("x should exist");
        let y_node = sg
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "y"))
            .expect("y should exist");

        // x and y should be in different scopes — they should not share an
        // incoming edge from the same scope node
        let x_scope_edges: Vec<_> = sg.edges.iter().filter(|e| e.target == x_node.id).collect();
        let y_scope_edges: Vec<_> = sg.edges.iter().filter(|e| e.target == y_node.id).collect();

        let x_scopes: Vec<_> = x_scope_edges.iter().map(|e| e.source).collect();
        let y_scopes: Vec<_> = y_scope_edges.iter().map(|e| e.source).collect();

        // The source scopes of the edges pointing to x and y should be different
        let shared: Vec<_> = x_scopes.iter().filter(|s| y_scopes.contains(s)).collect();
        assert!(
            shared.is_empty(),
            "x and y should be in different scopes, but share scope(s): {shared:?}"
        );
    }

    #[test]
    fn extracts_singleton_method_heuristic() {
        let source = "class Foo\n  def self.bar\n  end\nend\n";
        let tree = parse_ruby(source);
        let graph = RubySupport
            .extract_heuristic(&tree, source, Path::new("test.rb"))
            .unwrap();

        assert!(graph.definitions.iter().any(|d| d.name == "bar"
            && d.kind == SymbolKind::Function
            && d.qualified_name == "Foo.self.bar"));
    }

    #[test]
    fn extracts_calls_within_method() {
        let source = "class Foo\n  def bar\n    baz\n  end\nend\n";
        let tree = parse_ruby(source);
        let graph = RubySupport
            .extract_heuristic(&tree, source, Path::new("test.rb"))
            .unwrap();

        assert!(
            graph.calls.iter().any(|c| c.callee_name == "baz"),
            "Should have call to baz, got: {:?}",
            graph.calls
        );
    }
}
