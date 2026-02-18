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
pub struct JavaScriptSupport;

impl LanguageSupport for JavaScriptSupport {
    fn id(&self) -> &'static str {
        "javascript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["js", "jsx", "mjs", "cjs"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
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

        walk_js_node(
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

fn walk_js_node(
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
                walk_js_children(node, source, path, context, defs, calls, imports);
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

    walk_js_children(node, source, path, context, defs, calls, imports);
}

fn walk_js_children(
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
        walk_js_node(child, source, path, context, defs, calls, imports);
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

    fn parse_js(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_and_class() {
        let source =
            "function greet() {\n    console.log('hi');\n}\n\nclass Foo {\n    bar() {}\n}\n";
        let tree = parse_js(source);
        let graph = JavaScriptSupport
            .extract_heuristic(&tree, source, Path::new("test.js"))
            .unwrap();

        let fn_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .collect();
        assert_eq!(fn_defs.len(), 2);
        assert_eq!(fn_defs[0].name, "greet");
        assert_eq!(fn_defs[1].qualified_name, "Foo.bar");
    }

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_js(source);
        JavaScriptSupport
            .build_scope_graph(&tree, source, Path::new("test.js"))
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
        let sg = build_scope("function greet() {}\nclass Foo { bar() {} }\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"greet"), "Should have greet, got: {defs:?}");
        assert!(defs.contains(&"Foo"), "Should have Foo, got: {defs:?}");
        assert!(
            defs.contains(&"bar"),
            "Should have method bar, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_es_module_import() {
        let sg = build_scope("import { foo } from './bar';\n");
        let defs = pop_symbols(&sg);
        assert!(defs.contains(&"foo"), "Should bind foo, got: {defs:?}");
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should reference foo, got: {refs:?}");
    }

    #[test]
    fn scope_graph_arrow_function() {
        let sg = build_scope("const handler = (req, res) => { process(req); };\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"handler"),
            "Should have handler, got: {defs:?}"
        );
        assert!(
            defs.contains(&"req"),
            "Should have param req, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"process"),
            "Should reference process, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "function helper() {}\nhelper();\n";
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
            let tree = parse_js(source_a);
            JavaScriptSupport
                .build_scope_graph(&tree, source_a, Path::new("a.js"))
                .unwrap()
                .unwrap()
        };
        let source_b = "export function greet() {}\n";
        let sg_b = {
            let tree = parse_js(source_b);
            JavaScriptSupport
                .build_scope_graph(&tree, source_b, Path::new("b.js"))
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
                r.symbol == "greet" && r.definition_file == std::path::PathBuf::from("b.js")
            })
            .collect();
        assert!(
            !cross_file.is_empty(),
            "Should resolve cross-file, got: {resolved:?}"
        );
    }
}
