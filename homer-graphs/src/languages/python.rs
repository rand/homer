use std::path::Path;

use crate::{
    DocCommentData, DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport,
    ResolutionTier, Result, SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{child_by_field, dotted_name, hash_string, node_range, node_text};

#[derive(Debug)]
pub struct PythonSupport;

impl LanguageSupport for PythonSupport {
    fn id(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }

    fn tier(&self) -> ResolutionTier {
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
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

        walk_python_node(
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

fn walk_python_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    path: &Path,
    context: &mut Vec<String>,
    defs: &mut Vec<HeuristicDef>,
    calls: &mut Vec<HeuristicCall>,
    imports: &mut Vec<HeuristicImport>,
) {
    match node.kind() {
        "function_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_python_docstring(node, source);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname.clone(),
                    kind: SymbolKind::Function,
                    span: node_range(node),
                    doc_comment: doc,
                });

                // Walk body for calls
                if let Some(body) = child_by_field(node, "body") {
                    context.push(name);
                    extract_python_calls(body, source, &dotted_name(context, ""), calls);
                    walk_python_children(node, source, path, context, defs, calls, imports);
                    context.pop();
                    return;
                }
            }
        }
        "class_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source).to_string();
                let qname = dotted_name(context, &name);
                let doc = extract_python_docstring(node, source);

                defs.push(HeuristicDef {
                    name: name.clone(),
                    qualified_name: qname,
                    kind: SymbolKind::Type,
                    span: node_range(node),
                    doc_comment: doc,
                });

                context.push(name);
                walk_python_children(node, source, path, context, defs, calls, imports);
                context.pop();
                return;
            }
        }
        "import_statement" => {
            // import foo, bar
            let text = node_text(node, source);
            let names = text.strip_prefix("import ").unwrap_or(text).trim();
            for name in names.split(',') {
                imports.push(HeuristicImport {
                    from_path: path.to_path_buf(),
                    imported_name: name.trim().to_string(),
                    target_path: None,
                    confidence: 0.9,
                });
            }
        }
        "import_from_statement" => {
            // from foo import bar, baz
            let text = node_text(node, source);
            imports.push(HeuristicImport {
                from_path: path.to_path_buf(),
                imported_name: text.to_string(),
                target_path: None,
                confidence: 0.9,
            });
        }
        "call" => {
            if let Some(func) = child_by_field(node, "function") {
                let target = node_text(func, source).to_string();
                let scope = if context.is_empty() {
                    "<module>".to_string()
                } else {
                    context.join(".")
                };
                calls.push(HeuristicCall {
                    caller: scope,
                    callee_name: target,
                    span: node_range(node),
                    confidence: 0.7,
                });
            }
        }
        _ => {}
    }

    walk_python_children(node, source, path, context, defs, calls, imports);
}

fn walk_python_children(
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
        walk_python_node(child, source, path, context, defs, calls, imports);
    }
}

fn extract_python_calls(
    body: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        extract_python_calls_recursive(child, source, caller, calls);
    }
}

fn extract_python_calls_recursive(
    node: tree_sitter::Node<'_>,
    source: &str,
    caller: &str,
    calls: &mut Vec<HeuristicCall>,
) {
    if node.kind() == "call" {
        if let Some(func) = child_by_field(node, "function") {
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
        extract_python_calls_recursive(child, source, caller, calls);
    }
}

/// Extract Python docstring from the first statement of a function/class body.
fn extract_python_docstring(node: tree_sitter::Node<'_>, source: &str) -> Option<DocCommentData> {
    let body = child_by_field(node, "body")?;
    let mut cursor = body.walk();
    let first_stmt = body.children(&mut cursor).next()?;

    if first_stmt.kind() != "expression_statement" {
        return None;
    }

    let mut inner_cursor = first_stmt.walk();
    let expr = first_stmt.children(&mut inner_cursor).next()?;

    if expr.kind() != "string" {
        return None;
    }

    let text = node_text(expr, source);
    // Strip triple quotes
    let content = text
        .strip_prefix("\"\"\"")
        .and_then(|s| s.strip_suffix("\"\"\""))
        .or_else(|| text.strip_prefix("'''").and_then(|s| s.strip_suffix("'''")))
        .unwrap_or(text)
        .trim()
        .to_string();

    if content.is_empty() {
        return None;
    }

    // Detect docstring style
    let style = if content.contains(":param ") || content.contains(":type ") {
        DocStyle::Sphinx
    } else if content.contains("Args:") || content.contains("Returns:") {
        DocStyle::Google
    } else if content.contains("Parameters\n") || content.contains("----------") {
        DocStyle::Numpy
    } else {
        DocStyle::Other("python".to_string())
    };

    Some(DocCommentData {
        content_hash: hash_string(&content),
        text: content,
        style,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_python(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn extracts_function_and_class() {
        let source = r#"
def hello():
    """Says hello."""
    print("hi")

class Greeter:
    def greet(self):
        pass
"#;
        let tree = parse_python(source);
        let lang = PythonSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.py"))
            .unwrap();

        let fn_defs: Vec<_> = graph
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .collect();
        assert_eq!(fn_defs.len(), 2);
        assert_eq!(fn_defs[0].name, "hello");
        assert!(fn_defs[0].doc_comment.is_some());
        assert_eq!(fn_defs[1].qualified_name, "Greeter.greet");
    }

    #[test]
    fn extracts_imports() {
        let source = r"
import os
from pathlib import Path
";
        let tree = parse_python(source);
        let lang = PythonSupport;
        let graph = lang
            .extract_heuristic(&tree, source, Path::new("test.py"))
            .unwrap();

        assert_eq!(graph.imports.len(), 2);
        assert_eq!(graph.imports[0].imported_name, "os");
    }
}
