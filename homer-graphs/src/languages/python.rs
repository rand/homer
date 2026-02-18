use std::path::Path;

use crate::scope_graph::{FileScopeGraph, ScopeNodeId};
use crate::{
    DocCommentData, DocStyle, HeuristicCall, HeuristicDef, HeuristicGraph, HeuristicImport,
    ResolutionTier, Result, SymbolKind,
};

use super::LanguageSupport;
use super::helpers::{
    ScopeGraphBuilder, child_by_field, dotted_name, find_child_by_kind, hash_string, node_range,
    node_text,
};

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
        ResolutionTier::Precise
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
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

        walk_scope(
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

// ── Scope graph construction ─────────────────────────────────────────
//
// Walks the tree-sitter AST building a scope graph with:
// - PopSymbol for definitions (functions, classes, params, imports, assignments)
// - PushSymbol for references (call sites)
// - Scope nodes for function/class bodies (Python LEGB scoping)
// - ImportScope for cross-file imports
// - Exports for module-level definitions

/// Walk children of a node, dispatching each to the appropriate scope handler.
fn walk_scope(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                scope_function_def(child, source, scope, builder, module_defs, is_module_level);
            }
            "class_definition" => {
                scope_class_def(child, source, scope, builder, module_defs, is_module_level);
            }
            "decorated_definition" => {
                // Unwrap decorator to get the inner definition
                let inner = find_child_by_kind(child, "function_definition")
                    .or_else(|| find_child_by_kind(child, "class_definition"));
                if let Some(def) = inner {
                    match def.kind() {
                        "function_definition" => {
                            scope_function_def(
                                def,
                                source,
                                scope,
                                builder,
                                module_defs,
                                is_module_level,
                            );
                        }
                        _ => {
                            scope_class_def(
                                def,
                                source,
                                scope,
                                builder,
                                module_defs,
                                is_module_level,
                            );
                        }
                    }
                }
            }
            "import_statement" => {
                scope_import(child, source, scope, builder, module_defs, is_module_level);
            }
            "import_from_statement" => {
                scope_from_import(child, source, scope, builder, module_defs, is_module_level);
            }
            "call" => {
                scope_call(child, source, scope, builder);
                walk_scope(child, source, scope, builder, module_defs, false);
            }
            "assignment" if is_module_level => {
                scope_module_assignment(child, source, scope, builder, module_defs);
                walk_scope(child, source, scope, builder, module_defs, false);
            }
            _ => {
                // Control flow (if/for/while/with/try) — no new scope in Python.
                // Recurse preserving current scope and module-level flag.
                walk_scope(child, source, scope, builder, module_defs, is_module_level);
            }
        }
    }
}

fn scope_function_def(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
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
        module_defs.push(def_id);
    }

    // Create child scope for function body (linked to parent for LEGB lookup)
    let func_scope = builder.add_scope(scope, Some(node_range(node)));

    // Extract parameters as definitions in the function scope
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }

    // Walk function body in the new scope
    if let Some(body) = child_by_field(node, "body") {
        walk_scope(body, source, func_scope, builder, module_defs, false);
    }
}

fn scope_class_def(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
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
        module_defs.push(def_id);
    }

    // Create child scope for class body
    let class_scope = builder.add_scope(scope, Some(node_range(node)));

    if let Some(body) = child_by_field(node, "body") {
        walk_scope(body, source, class_scope, builder, module_defs, false);
    }
}

fn scope_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let full_name = node_text(child, source);
                // `import os.path` binds local name "os" (first component)
                let local_name = full_name.split('.').next().unwrap_or(full_name);
                let import_scope = builder.add_import_scope();
                builder.add_import_reference(import_scope, full_name, Some(node_range(child)));
                let def_id = builder.add_definition(
                    scope,
                    local_name,
                    Some(node_range(child)),
                    Some(SymbolKind::Module),
                );
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            "aliased_import" => {
                let name = child_by_field(child, "name").map_or("", |n| node_text(n, source));
                let alias = child_by_field(child, "alias").map_or(name, |n| node_text(n, source));
                let import_scope = builder.add_import_scope();
                builder.add_import_reference(import_scope, name, Some(node_range(child)));
                let def_id = builder.add_definition(
                    scope,
                    alias,
                    Some(node_range(child)),
                    Some(SymbolKind::Module),
                );
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            _ => {}
        }
    }
}

fn scope_from_import(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let import_scope = builder.add_import_scope();

    // Skip the module_name field node to avoid treating it as an imported name
    let module_name_node = child_by_field(node, "module_name");
    let module_name_id = module_name_node.map(|n| n.id());

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if Some(child.id()) == module_name_id {
            continue;
        }
        match child.kind() {
            "dotted_name" => {
                let name = node_text(child, source);
                builder.add_import_reference(import_scope, name, Some(node_range(child)));
                let def_id = builder.add_definition(scope, name, Some(node_range(child)), None);
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            "aliased_import" => {
                let name = child_by_field(child, "name").map_or("", |n| node_text(n, source));
                let alias = child_by_field(child, "alias").map_or(name, |n| node_text(n, source));
                builder.add_import_reference(import_scope, name, Some(node_range(child)));
                let def_id = builder.add_definition(scope, alias, Some(node_range(child)), None);
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            "wildcard_import" => {
                builder.add_import_reference(import_scope, "*", Some(node_range(child)));
            }
            _ => {}
        }
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
        "attribute" => child_by_field(func, "attribute").map(|a| node_text(a, source).to_string()),
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

fn scope_module_assignment(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
) {
    let Some(left) = child_by_field(node, "left") else {
        return;
    };
    if left.kind() == "identifier" {
        let name = node_text(left, source);
        let def_id = builder.add_definition(
            scope,
            name,
            Some(node_range(left)),
            Some(SymbolKind::Variable),
        );
        module_defs.push(def_id);
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
        let name = match child.kind() {
            "identifier" => Some(node_text(child, source)),
            "default_parameter" | "typed_parameter" | "typed_default_parameter" => {
                child_by_field(child, "name").map(|n| node_text(n, source))
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                find_child_by_kind(child, "identifier").map(|n| node_text(n, source))
            }
            _ => None,
        };
        if let Some(name) = name {
            builder.add_definition(
                func_scope,
                name,
                Some(node_range(child)),
                Some(SymbolKind::Variable),
            );
        }
    }
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

    // ── Scope graph tests ──────────────────────────────────────────

    use crate::scope_graph::{ScopeGraph, ScopeNodeKind};

    fn build_scope(source: &str) -> FileScopeGraph {
        let tree = parse_python(source);
        let lang = PythonSupport;
        lang.build_scope_graph(&tree, source, Path::new("test.py"))
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
    fn scope_graph_function_def_creates_pop_symbol() {
        let sg = build_scope("def hello():\n    pass\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"hello"),
            "Should have PopSymbol for hello, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_class_def_creates_pop_symbol() {
        let sg = build_scope("class Greeter:\n    pass\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Greeter"),
            "Should have PopSymbol for Greeter, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_call_creates_push_symbol() {
        let source = "def foo():\n    pass\nfoo()\n";
        let sg = build_scope(source);
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"foo"),
            "Should have PushSymbol for foo(), got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_import_creates_local_binding() {
        let sg = build_scope("import os\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"os"),
            "import should create PopSymbol for os, got: {defs:?}"
        );
        let imports: Vec<_> = sg
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, ScopeNodeKind::ImportScope))
            .collect();
        assert!(!imports.is_empty(), "Should have ImportScope node");
    }

    #[test]
    fn scope_graph_from_import_creates_local_binding() {
        let sg = build_scope("from pathlib import Path\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"Path"),
            "from-import should create PopSymbol for Path, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Path"),
            "from-import should create PushSymbol for cross-file ref, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_aliased_import() {
        let sg = build_scope("from pathlib import Path as P\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"P"),
            "aliased import should bind local name P, got: {defs:?}"
        );
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"Path"),
            "aliased import should reference original name Path, got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_params_create_definitions() {
        let sg = build_scope("def greet(name, greeting='hi'):\n    pass\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"name"),
            "Should have param 'name', got: {defs:?}"
        );
        assert!(
            defs.contains(&"greeting"),
            "Should have param 'greeting', got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_module_assignment_exported() {
        let sg = build_scope("MAX_RETRIES = 5\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"MAX_RETRIES"),
            "Should have PopSymbol for assignment, got: {defs:?}"
        );
        assert!(
            sg.export_nodes.iter().any(|&id| {
                sg.nodes.iter().any(|n| n.id == id && matches!(&n.kind, ScopeNodeKind::PopSymbol { symbol } if symbol == "MAX_RETRIES"))
            }),
            "MAX_RETRIES should be in export_nodes"
        );
    }

    #[test]
    fn scope_graph_within_file_resolution() {
        let source = "def helper():\n    pass\nhelper()\n";
        let sg = build_scope(source);

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();

        assert!(
            resolved.iter().any(|r| r.symbol == "helper"),
            "Call to helper() should resolve to def helper, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_class_method_resolution() {
        let source = "class Foo:\n    def bar(self):\n        pass\n    def baz(self):\n        self.bar()\n";
        let sg = build_scope(source);

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();

        // self.bar() creates PushSymbol("bar"), which should resolve via
        // baz scope → class scope → PopSymbol("bar")
        let bar_refs: Vec<_> = resolved.iter().filter(|r| r.symbol == "bar").collect();
        assert!(
            !bar_refs.is_empty(),
            "self.bar() should resolve to method bar, resolved: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_nested_function_resolution() {
        let source = "def outer():\n    def inner():\n        pass\n    inner()\n";
        let sg = build_scope(source);

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg);
        let resolved = scope_graph.resolve_all();

        assert!(
            resolved.iter().any(|r| r.symbol == "inner"),
            "inner() should resolve to def inner, got: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_cross_file_resolution() {
        // File A: from b import greet; greet()
        let source_a = "from b import greet\ngreet()\n";
        let sg_a = {
            let tree = parse_python(source_a);
            PythonSupport
                .build_scope_graph(&tree, source_a, Path::new("a.py"))
                .unwrap()
                .unwrap()
        };

        // File B: def greet(): pass
        let source_b = "def greet():\n    pass\n";
        let sg_b = {
            let tree = parse_python(source_b);
            PythonSupport
                .build_scope_graph(&tree, source_b, Path::new("b.py"))
                .unwrap()
                .unwrap()
        };

        let mut scope_graph = ScopeGraph::new();
        scope_graph.add_file_graph(&sg_a);
        scope_graph.add_file_graph(&sg_b);
        let resolved = scope_graph.resolve_all();

        // The import's PushSymbol("greet") should resolve to b.py's PopSymbol("greet")
        let cross_file: Vec<_> = resolved
            .iter()
            .filter(|r| r.symbol == "greet" && r.definition_file == std::path::Path::new("b.py"))
            .collect();
        assert!(
            !cross_file.is_empty(),
            "import greet should resolve cross-file, all resolved: {resolved:?}"
        );
    }

    #[test]
    fn scope_graph_decorated_function() {
        let sg = build_scope("@staticmethod\ndef compute():\n    pass\n");
        let defs = pop_symbols(&sg);
        assert!(
            defs.contains(&"compute"),
            "Decorated function should create PopSymbol, got: {defs:?}"
        );
    }

    #[test]
    fn scope_graph_method_call_via_attribute() {
        let source = "def run():\n    obj.process()\n";
        let sg = build_scope(source);
        let refs = push_symbols(&sg);
        assert!(
            refs.contains(&"process"),
            "obj.process() should create PushSymbol for 'process', got: {refs:?}"
        );
    }

    #[test]
    fn scope_graph_nested_calls() {
        let source = "foo(bar(baz()))\n";
        let sg = build_scope(source);
        let refs = push_symbols(&sg);
        assert!(refs.contains(&"foo"), "Should find foo, got: {refs:?}");
        assert!(refs.contains(&"bar"), "Should find bar, got: {refs:?}");
        assert!(refs.contains(&"baz"), "Should find baz, got: {refs:?}");
    }
}
