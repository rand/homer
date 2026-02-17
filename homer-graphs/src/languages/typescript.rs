use std::path::Path;

use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    child_by_field, dotted_name, extract_block_doc_comment, node_range, node_text,
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
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
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
}
