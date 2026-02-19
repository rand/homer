// Query engine — shared entity lookup, name resolution, and graph traversal.
//
// Used by both the CLI `query` command and the MCP `homer_query` tool.

use std::collections::HashSet;

use crate::store::HomerStore;
use crate::types::{HyperedgeKind, Node, NodeFilter, NodeId, NodeKind};

/// Resolve a `NodeId` to its display name, or "node:{id}" if not found.
pub async fn resolve_name(store: &dyn HomerStore, node_id: NodeId) -> String {
    store
        .get_node(node_id)
        .await
        .ok()
        .flatten()
        .map_or_else(|| format!("node:{}", node_id.0), |n| n.name)
}

/// Find an entity by name: tries exact match by kind, then substring match.
pub async fn find_entity(store: &dyn HomerStore, name: &str) -> crate::error::Result<Option<Node>> {
    for kind in [
        NodeKind::File,
        NodeKind::Function,
        NodeKind::Type,
        NodeKind::Module,
        NodeKind::Document,
    ] {
        if let Some(node) = store.get_node_by_name(kind, name).await? {
            return Ok(Some(node));
        }
    }

    // Partial match fallback
    for kind in [
        NodeKind::File,
        NodeKind::Function,
        NodeKind::Type,
        NodeKind::Module,
    ] {
        let nodes = store
            .find_nodes(&NodeFilter {
                kind: Some(kind),
                ..Default::default()
            })
            .await?;
        for node in nodes {
            if node.name.contains(name) || node.name.ends_with(name) {
                return Ok(Some(node));
            }
        }
    }

    Ok(None)
}

/// Resolve call-graph edges for a node, returning (incoming callers, outgoing callees).
pub async fn resolve_call_edges(store: &dyn HomerStore, id: NodeId) -> (Vec<String>, Vec<String>) {
    let Ok(edges) = store.get_edges_involving(id).await else {
        return (Vec::new(), Vec::new());
    };
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    for edge in &edges {
        if edge.kind != HyperedgeKind::Calls {
            continue;
        }
        for m in &edge.members {
            if m.node_id == id {
                continue;
            }
            let name = resolve_name(store, m.node_id).await;
            if m.role == "caller" {
                incoming.push(name);
            } else if m.role == "callee" {
                outgoing.push(name);
            }
        }
    }
    (incoming, outgoing)
}

/// Resolve names of nodes related via a specific edge kind.
pub async fn resolve_related_names(
    store: &dyn HomerStore,
    id: NodeId,
    kind: HyperedgeKind,
) -> Vec<String> {
    let Ok(edges) = store.get_edges_involving(id).await else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for edge in &edges {
        if edge.kind != kind {
            continue;
        }
        for m in &edge.members {
            if m.node_id == id {
                continue;
            }
            names.push(resolve_name(store, m.node_id).await);
        }
    }
    names
}

/// BFS traversal collecting directed neighbors at each depth level.
pub async fn collect_neighbors_bfs(
    store: &dyn HomerStore,
    start: NodeId,
    self_role: &str,
    neighbor_role: &str,
    max_depth: u32,
) -> crate::error::Result<Vec<(u32, String)>> {
    let mut result = Vec::new();
    let mut frontier = vec![start];
    let mut visited = HashSet::new();
    visited.insert(start);

    for depth in 1..=max_depth {
        let mut next_frontier = Vec::new();
        for &nid in &frontier {
            let edges = store.get_edges_involving(nid).await?;
            for edge in edges.iter().filter(|e| e.kind == HyperedgeKind::Calls) {
                let self_m = edge.members.iter().find(|m| m.role == self_role);
                let neighbor_m = edge.members.iter().find(|m| m.role == neighbor_role);
                if let (Some(s), Some(n)) = (self_m, neighbor_m) {
                    if s.node_id == nid && visited.insert(n.node_id) {
                        let name = resolve_name(store, n.node_id).await;
                        result.push((depth, name));
                        next_frontier.push(n.node_id);
                    }
                }
            }
        }
        frontier = next_frontier;
    }
    Ok(result)
}

/// Parse a user-provided string into a `NodeKind`.
pub fn parse_node_kind(s: &str) -> Option<NodeKind> {
    match s.to_lowercase().as_str() {
        "function" | "fn" => Some(NodeKind::Function),
        "type" | "struct" | "class" => Some(NodeKind::Type),
        "file" => Some(NodeKind::File),
        "module" | "dir" | "directory" => Some(NodeKind::Module),
        "commit" => Some(NodeKind::Commit),
        "contributor" | "author" => Some(NodeKind::Contributor),
        "pr" | "pullrequest" => Some(NodeKind::PullRequest),
        "issue" => Some(NodeKind::Issue),
        "dep" | "dependency" => Some(NodeKind::ExternalDep),
        "document" | "doc" => Some(NodeKind::Document),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_node_kind_variants() {
        assert_eq!(parse_node_kind("function"), Some(NodeKind::Function));
        assert_eq!(parse_node_kind("fn"), Some(NodeKind::Function));
        assert_eq!(parse_node_kind("type"), Some(NodeKind::Type));
        assert_eq!(parse_node_kind("file"), Some(NodeKind::File));
        assert_eq!(parse_node_kind("module"), Some(NodeKind::Module));
        assert_eq!(parse_node_kind("commit"), Some(NodeKind::Commit));
        assert_eq!(parse_node_kind("pr"), Some(NodeKind::PullRequest));
        assert_eq!(parse_node_kind("issue"), Some(NodeKind::Issue));
        assert_eq!(parse_node_kind("unknown"), None);
        assert_eq!(parse_node_kind("all"), None);
    }
}
