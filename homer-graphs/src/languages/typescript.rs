use std::path::Path;

use crate::scope_graph::FileScopeGraph;
use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, dotted_name, extract_block_doc_comment, node_range,
    node_text,
};

#[derive(Debug)]
pub struct TypeScriptSupport;

impl LanguageSupport for TypeScriptSupport {
    fn id(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn build_scope_graph(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &Path,
    ) -> Result<Option<FileScopeGraph>> {
        let mut builder = ScopeGraphBuilder::new(path);
        let root = builder.root();
        let mut module_defs = Vec::new();

        super::ecma_scope::walk_scope(
            tree.root_node(),
            source,
            root,
            &mut builder,
            &mut module_defs,
            true,
        );

        for def_id in &module_defs {
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

        walk_ts_node(
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

fn walk_ts_node(
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
                let doc = extract_block_doc_comment(node, source, DocStyle::Jsdoc);

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
        "class_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::Jsdoc),
                });

                context.push(name);
                walk_ts_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "method_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);

                defs.push(HeuristicDef {
                    name,
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::Jsdoc),
                });

                if let Some(body) = child_by_field(node, "body") {
                    extract_calls_recursive(body, source, &qname, calls);
                }
            }
        }
        "interface_declaration" | "type_alias_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: dotted_name(context, &name),
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: extract_block_doc_comment(node, source, DocStyle::Jsdoc),
                });
            }
        }
        "import_statement" => {
            imports.push(HeuristicImport {
                from_path: path.to_path_buf(),
                imported_name: node_text(node, source).to_string(),
                target_path: None,
                confidence: 0.9,
            });
        }
        _ => {}
    }

    walk_ts_children(node, source, path, context, defs, calls, imports);
}

fn walk_ts_children(
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
        walk_ts_node(child, source, path, context, defs, calls, imports);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ts(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_and_class() {
        let source = "function greet(name: string): void {\n    console.log(name);\n}\n\nclass Greeter {\n    greet() {}\n}\n";
        let tree = parse_ts(source);
        let graph = TypeScriptSupport
            .extract_heuristic(&tree, source, Path::new("test.ts"))
            .unwrap();

        let fn_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .collect();
        assert_eq!(fn_defs.len(), 2);
        assert_eq!(fn_defs[0].name, "greet");
        assert_eq!(fn_defs[1].qualified_name, "Greeter.greet");
    }

    #[test]
    fn extracts_imports() {
        let source = "import { foo } from './bar';";
        let tree = parse_ts(source);
        let graph = TypeScriptSupport
            .extract_heuristic(&tree, source, Path::new("test.ts"))
            .unwrap();
        assert_eq!(graph.imports.len(), 1);
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_ts(source);
        TypeScriptSupport
            .build_scope_graph(&tree, source, Path::new("test.ts"))
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
    fn scope_graph_function_and_class() {
        let sg =
            build_scope("function greet(name: string): void {}\nclass Greeter { greet() {} }\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"greet"),
            "Should have function greet, got: {defs:?}"
        );
        assert!(
            defs.contains(&"Greeter"),
            "Should have class Greeter, got: {defs:?}"
        );
        assert!(
            defs.contains(&"name"),
            "Should have param 'name', got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_named_import() {
        let sg = build_scope("import { foo, bar as baz } from './module';\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"foo"), "Should bind foo, got: {defs:?}");
        assert!(
            defs.contains(&"baz"),
            "Should bind alias baz, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should reference foo, got: {refs:?}");
        assert!(
            refs.contains(&"bar"),
            "Should reference original bar, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_default_import() {
        let sg = build_scope("import React from 'react';\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"React"), "Should bind React, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"default"),
            "Default import should ref 'default', got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_namespace_import() {
        let sg = build_scope("import * as utils from './utils';\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"utils"), "Should bind utils, got: {defs:?}");
    }

    #[test]
    fn scope_graph_export_function() {
        let sg = build_scope("export function compute(): number { return 0; }\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"compute"),
            "Should have compute, got: {defs:?}"
        );
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "compute"))
            }),
            "compute should be exported"
        );
    }

    #[test]
    fn scope_graph_export_default() {
        let sg = build_scope("export default 42;\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"default"),
            "Should have default export, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_interface_and_type() {
        let sg = build_scope("interface Props { name: string; }\ntype ID = string;\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Props"),
            "Should have interface Props, got: {defs:?}"
        );
        assert!(defs.contains(&"ID"), "Should have type ID, got: {defs:?}");
    }

    #[test]
    fn scope_graph_arrow_function() {
        let sg = build_scope("const greet = (name: string) => { console.log(name); };\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"greet"),
            "Should have arrow fn 'greet', got: {defs:?}"
        );
        assert!(
            defs.contains(&"name"),
            "Should have param 'name', got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_call_expression() {
        let source = "function foo() {}\nfoo();\n";
        let sg = build_scope(source);
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should have PushSymbol for foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "function helper(): void {}\nhelper();\n";
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
        let source_a = "import { greet } from './b';\ngreet();\n";
        let sg_a = {
            let tree = parse_ts(source_a);
            TypeScriptSupport
                .build_scope_graph(&tree, source_a, Path::new("a.ts"))
                .unwrap()
                .unwrap()
        };
        let source_b = "export function greet(): void {}\n";
        let sg_b = {
            let tree = parse_ts(source_b);
            TypeScriptSupport
                .build_scope_graph(&tree, source_b, Path::new("b.ts"))
                .unwrap()
                .unwrap()
        };

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg_a);
        scope_graph.add_file_graph(&sg_b);
        let resolved = scope_graph.resolve_all();

        let cross_file: Vec<_> = resolved
            .iter()
            .filter(|r| r.symbol == "greet" && r.definition_file == std::path::Path::new("b.ts"))
            .collect();
        assert!(
            !cross_file.is_empty(),
            "import greet should resolve cross-file, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_method_in_class() {
        let sg = build_scope("class Foo {\n  bar() { this.baz(); }\n  baz() {}\n}\n");
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"baz"),
            "this.baz() should create PushSymbol for baz, got: {refs:?}"
        );
    }
}
