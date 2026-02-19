// Shared helpers for forge extractors (GitHub, GitLab).

use std::collections::HashMap;

use chrono::Utc;

use crate::store::HomerStore;
use crate::types::{Node, NodeId, NodeKind};

use super::traits::ExtractStats;

/// Ensure a Contributor node exists, return its ID.
pub async fn ensure_contributor(
    store: &dyn HomerStore,
    stats: &mut ExtractStats,
    login: &str,
) -> crate::error::Result<NodeId> {
    if let Some(node) = store.get_node_by_name(NodeKind::Contributor, login).await? {
        return Ok(node.id);
    }

    let id = store
        .upsert_node(&Node {
            id: NodeId(0),
            kind: NodeKind::Contributor,
            name: login.to_string(),
            content_hash: None,
            last_extracted: Utc::now(),
            metadata: HashMap::new(),
        })
        .await?;
    stats.nodes_created += 1;
    Ok(id)
}

/// Parse issue cross-references from PR/MR description text.
/// Matches patterns like "fixes #123", "closes #456", "resolves #789".
pub fn parse_issue_refs(text: &str) -> Vec<u64> {
    let lower = text.to_lowercase();
    let mut refs = Vec::new();

    let patterns = [
        "close ",
        "closes ",
        "closed ",
        "fix ",
        "fixes ",
        "fixed ",
        "resolve ",
        "resolves ",
        "resolved ",
    ];

    for pattern in &patterns {
        let mut search = lower.as_str();
        while let Some(pos) = search.find(pattern) {
            let after = &search[pos + pattern.len()..];
            if let Some(num) = extract_issue_number(after) {
                if !refs.contains(&num) {
                    refs.push(num);
                }
            }
            search = &search[pos + pattern.len()..];
        }
    }

    refs
}

/// Extract an issue number after a keyword, e.g., "#123" or "org/repo#123".
pub fn extract_issue_number(text: &str) -> Option<u64> {
    let text = text.trim_start();
    let text = if let Some(rest) = text.strip_prefix('#') {
        rest
    } else {
        // Could be "org/repo#123" — skip to #
        let (_, after) = text.split_once('#')?;
        after
    };

    let num_str: String = text.chars().take_while(char::is_ascii_digit).collect();
    if num_str.is_empty() {
        return None;
    }
    num_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_refs_basic() {
        let refs = parse_issue_refs("This fixes #42 and closes #99");
        assert!(refs.contains(&42));
        assert!(refs.contains(&99));
    }

    #[test]
    fn parse_refs_case_insensitive() {
        let refs = parse_issue_refs("FIXES #10, Resolves #20");
        assert!(refs.contains(&10));
        assert!(refs.contains(&20));
    }

    #[test]
    fn parse_refs_no_duplicates() {
        let refs = parse_issue_refs("fixes #5, also fixes #5");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn parse_refs_org_repo_syntax() {
        let refs = parse_issue_refs("fixes org/repo#123");
        assert!(refs.contains(&123));
    }

    #[test]
    fn parse_refs_no_refs() {
        let refs = parse_issue_refs("This PR adds a feature");
        assert!(refs.is_empty());
    }

    #[test]
    fn extract_number_from_hash() {
        assert_eq!(extract_issue_number("#42"), Some(42));
        assert_eq!(extract_issue_number("  #100"), Some(100));
        assert_eq!(extract_issue_number("#abc"), None);
    }

    #[test]
    fn extract_number_no_hash() {
        assert_eq!(extract_issue_number("no hash"), None);
    }
}
