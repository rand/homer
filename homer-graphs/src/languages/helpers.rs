use tree_sitter::Node;

use crate::{DocCommentData, DocStyle, TextRange};

/// Extract the source text for a tree-sitter node.
pub fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// Find the first child with a specific kind.
pub fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Find a child by field name.
pub fn child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

/// Extract a doc comment from comment nodes immediately preceding a definition.
pub fn extract_doc_comment_above(
    node: Node<'_>,
    source: &str,
    style: DocStyle,
    prefix: &str,
) -> Option<DocCommentData> {
    let mut comments = Vec::new();
    let mut current = node;

    // Walk backwards through siblings collecting comment lines
    while let Some(prev) = current.prev_sibling() {
        if prev.kind() == "line_comment" || prev.kind() == "comment" {
            let text = node_text(prev, source);
            if text.starts_with(prefix) {
                let stripped = text.strip_prefix(prefix).unwrap_or(text).trim();
                comments.push(stripped.to_string());
                current = prev;
                continue;
            }
        }
        break;
    }

    if comments.is_empty() {
        return None;
    }

    // Reverse since we collected bottom-to-top
    comments.reverse();
    let text = comments.join("\n");
    let content_hash = hash_string(&text);

    Some(DocCommentData {
        text,
        content_hash,
        style,
    })
}

/// Extract a block doc comment (/** ... */) from the preceding sibling.
pub fn extract_block_doc_comment(
    node: Node<'_>,
    source: &str,
    style: DocStyle,
) -> Option<DocCommentData> {
    let prev = node.prev_sibling()?;
    if prev.kind() != "comment" && prev.kind() != "block_comment" {
        return None;
    }

    let text = node_text(prev, source);
    if !text.starts_with("/**") {
        return None;
    }

    // Strip /** prefix and */ suffix, clean up * at start of lines
    let inner = text
        .strip_prefix("/**")
        .unwrap_or(text)
        .strip_suffix("*/")
        .unwrap_or(text)
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("* ")
                .or(trimmed.strip_prefix('*'))
                .unwrap_or(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if inner.is_empty() {
        return None;
    }

    let content_hash = hash_string(&inner);
    Some(DocCommentData {
        text: inner,
        content_hash,
        style,
    })
}

/// Build a qualified name from a module context stack using `::` separator (Rust).
pub fn qualified_name(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", context.join("::"))
    }
}

/// Build a qualified name from a module context stack using `.` separator (most languages).
pub fn dotted_name(context: &[String], name: &str) -> String {
    if context.is_empty() {
        name.to_string()
    } else {
        format!("{}.{name}", context.join("."))
    }
}

/// Convert a tree-sitter node to a `TextRange`.
pub fn node_range(node: Node<'_>) -> TextRange {
    node.range().into()
}

/// Simple string hash for content dedup.
pub fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
