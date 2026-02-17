// Shared ES module scope graph construction for TypeScript and JavaScript.
//
// Handles all ECMAScript patterns common to both languages:
// - function_declaration, class_declaration, method_definition
// - import_statement (named, namespace, default imports)
// - export_statement (declarations, named exports, default exports)
// - call_expression, lexical_declaration, formal_parameters
// - TypeScript-specific: interface_declaration, type_alias_declaration, enum_declaration

use crate::scope_graph::ScopeNodeId;
use crate::SymbolKind;

use super::helpers::{
    ScopeGraphBuilder, child_by_field, find_child_by_kind, node_range, node_text,
};

/// Walk children of a node, dispatching to the appropriate scope handler.
pub fn walk_scope(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_node(child, source, scope, builder, module_defs, is_module_level);
    }
}

fn dispatch_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            scope_function_decl(node, source, scope, builder, module_defs, is_module_level);
        }
        "class_declaration" => {
            scope_class_decl(node, source, scope, builder, module_defs, is_module_level);
        }
        "method_definition" => {
            scope_method_def(node, source, scope, builder);
        }
        "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
            scope_type_decl(node, source, scope, builder, module_defs, is_module_level);
        }
        "import_statement" => {
            scope_import(node, source, scope, builder, module_defs, is_module_level);
        }
        "export_statement" => {
            scope_export(node, source, scope, builder, module_defs, is_module_level);
        }
        "call_expression" => {
            scope_call(node, source, scope, builder);
            walk_scope(node, source, scope, builder, module_defs, false);
        }
        "lexical_declaration" | "variable_declaration" if is_module_level => {
            scope_var_decl(node, source, scope, builder, module_defs);
        }
        _ => {
            walk_scope(node, source, scope, builder, module_defs, is_module_level);
        }
    }
}

fn scope_function_decl(
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
    let def_id =
        builder.add_definition(scope, name, Some(node_range(name_node)), Some(SymbolKind::Function));
    if is_module_level {
        module_defs.push(def_id);
    }

    let func_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, func_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        walk_scope(body, source, func_scope, builder, module_defs, false);
    }
}

fn scope_class_decl(
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
    let def_id =
        builder.add_definition(scope, name, Some(node_range(name_node)), Some(SymbolKind::Type));
    if is_module_level {
        module_defs.push(def_id);
    }

    let class_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(body) = child_by_field(node, "body") {
        walk_scope(body, source, class_scope, builder, module_defs, false);
    }
}

fn scope_method_def(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    let name = node_text(name_node, source);
    builder.add_definition(scope, name, Some(node_range(name_node)), Some(SymbolKind::Function));

    let method_scope = builder.add_scope(scope, Some(node_range(node)));
    if let Some(params) = child_by_field(node, "parameters") {
        scope_params(params, source, method_scope, builder);
    }
    if let Some(body) = child_by_field(node, "body") {
        // Methods don't contribute to module_defs
        let mut ignored = Vec::new();
        walk_scope(body, source, method_scope, builder, &mut ignored, false);
    }
}

fn scope_type_decl(
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
    let def_id =
        builder.add_definition(scope, name, Some(node_range(name_node)), Some(SymbolKind::Type));
    if is_module_level {
        module_defs.push(def_id);
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
    let import_scope = builder.add_import_scope();

    let Some(clause) = find_child_by_kind(node, "import_clause") else {
        return; // Side-effect import: import './module'
    };

    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                // Default import: import foo from '...'
                let local_name = node_text(child, source);
                builder.add_import_reference(import_scope, "default", Some(node_range(child)));
                let def_id = builder.add_definition(scope, local_name, Some(node_range(child)), None);
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            "named_imports" => {
                scope_named_imports(
                    child, source, scope, import_scope, builder, module_defs, is_module_level,
                );
            }
            "namespace_import" => {
                let ns_name = find_child_by_kind(child, "identifier")
                    .map_or("*", |n| node_text(n, source));
                builder.add_import_reference(import_scope, "*", Some(node_range(child)));
                let def_id = builder.add_definition(
                    scope, ns_name, Some(node_range(child)), Some(SymbolKind::Module),
                );
                if is_module_level {
                    module_defs.push(def_id);
                }
            }
            _ => {}
        }
    }
}

fn scope_named_imports(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    import_scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_specifier" {
            let name = child_by_field(child, "name").map_or("", |n| node_text(n, source));
            let alias = child_by_field(child, "alias").map_or(name, |n| node_text(n, source));
            builder.add_import_reference(import_scope, name, Some(node_range(child)));
            let def_id = builder.add_definition(scope, alias, Some(node_range(child)), None);
            if is_module_level {
                module_defs.push(def_id);
            }
        }
    }
}

fn scope_export(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
    is_module_level: bool,
) {
    let before_len = module_defs.len();

    // Case 1: export <declaration>
    if let Some(decl) = child_by_field(node, "declaration") {
        dispatch_node(decl, source, scope, builder, module_defs, is_module_level);
    }

    // Case 2: export default <value>
    if let Some(value) = child_by_field(node, "value") {
        if matches!(
            value.kind(),
            "function_declaration" | "class_declaration" | "function"
        ) {
            dispatch_node(value, source, scope, builder, module_defs, is_module_level);
        }
        let def_id = builder.add_definition(scope, "default", Some(node_range(node)), None);
        if is_module_level {
            module_defs.push(def_id);
        }
    }

    // Case 3: export { foo, bar }
    if let Some(clause) = find_child_by_kind(node, "export_clause") {
        scope_export_clause(clause, source, scope, builder, module_defs);
    }

    // Mark all new defs from this export as exported
    for def_id in &module_defs[before_len..] {
        builder.mark_exported(*def_id);
    }
}

fn scope_export_clause(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "export_specifier" {
            let name = child_by_field(child, "name").map_or("", |n| node_text(n, source));
            if !name.is_empty() {
                let def_id = builder.add_definition(scope, name, Some(node_range(child)), None);
                module_defs.push(def_id);
            }
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
        "member_expression" => {
            child_by_field(func, "property").map(|p| node_text(p, source).to_string())
        }
        _ => None,
    };
    if let Some(name) = target {
        builder.add_reference(scope, &name, Some(node_range(func)), Some(SymbolKind::Function));
    }
}

fn scope_var_decl(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            scope_single_var(child, source, scope, builder, module_defs);
        }
    }
}

fn scope_single_var(
    node: tree_sitter::Node<'_>,
    source: &str,
    scope: ScopeNodeId,
    builder: &mut ScopeGraphBuilder,
    module_defs: &mut Vec<ScopeNodeId>,
) {
    let Some(name_node) = child_by_field(node, "name") else {
        return;
    };
    if name_node.kind() != "identifier" {
        return; // Destructuring — skip for now
    }
    let name = node_text(name_node, source);
    let is_arrow = child_by_field(node, "value").is_some_and(|v| v.kind() == "arrow_function");
    let kind = if is_arrow { SymbolKind::Function } else { SymbolKind::Variable };
    let def_id = builder.add_definition(scope, name, Some(node_range(name_node)), Some(kind));
    module_defs.push(def_id);

    // Arrow function body gets its own scope
    if let Some(value) = child_by_field(node, "value") {
        if value.kind() == "arrow_function" {
            let func_scope = builder.add_scope(scope, Some(node_range(value)));
            if let Some(params) = child_by_field(value, "parameters") {
                scope_params(params, source, func_scope, builder);
            }
            if let Some(body) = child_by_field(value, "body") {
                walk_scope(body, source, func_scope, builder, module_defs, false);
            }
        }
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
            "required_parameter" | "optional_parameter" => child_by_field(child, "pattern")
                .filter(|n| n.kind() == "identifier")
                .map(|n| node_text(n, source)),
            "rest_pattern" | "rest_parameter" => {
                find_child_by_kind(child, "identifier").map(|n| node_text(n, source))
            }
            // assignment_pattern: param with default value (e.g., x = 5)
            "assignment_pattern" => child_by_field(child, "left")
                .filter(|n| n.kind() == "identifier")
                .map(|n| node_text(n, source)),
            _ => None,
        };
        if let Some(name) = name {
            builder.add_definition(
                func_scope, name, Some(node_range(child)), Some(SymbolKind::Variable),
            );
        }
    }
}
