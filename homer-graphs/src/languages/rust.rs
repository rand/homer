use std::path::Path;

use crate::{
    DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport, ResolutionTier, Result,
    SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    child_by_field, extract_doc_comment_above, node_range, node_text, qualified_name,
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
        ResolutionTier::Heuristic
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
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
