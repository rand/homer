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
pub struct JavaSupport;

impl LanguageSupport for JavaSupport {
    fn id(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
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
}
