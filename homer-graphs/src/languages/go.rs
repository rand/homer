use std::path::Path;

use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    child_by_field, dotted_name, extract_doc_comment_above, find_child_by_kind, node_range,
    node_text,
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
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
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
