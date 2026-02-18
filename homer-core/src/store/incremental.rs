// Incrementality tracking — content-hash comparison, invalidation cascades.
//
// Provides helpers for extractors and analyzers to skip unchanged work and
// cascade invalidation when nodes change.

use std::collections::HashSet;

use tracing::debug;

use crate::store::HomerStore;
use crate::types::{Node, NodeId};

/// Check whether a node's content has changed since last extraction.
///
/// Compares `new_hash` against the stored `content_hash`.
/// Returns `true` if the hash differs (or node doesn't exist yet).
pub async fn has_content_changed(
    store: &dyn HomerStore,
    node_id: NodeId,
    new_hash: u64,
) -> crate::error::Result<bool> {
    let existing = store.get_node(node_id).await?;
    match existing {
        Some(node) => Ok(node.content_hash != Some(new_hash)),
        None => Ok(true),
    }
}

/// Upsert a node and, if its content hash changed, invalidate its analysis results.
///
/// Returns `(node_id, changed)` — the stored node ID and whether invalidation occurred.
pub async fn upsert_if_changed(
    store: &dyn HomerStore,
    node: &Node,
) -> crate::error::Result<(NodeId, bool)> {
    // Check existing hash before upserting
    let existing = store
        .get_node_by_name(node.kind.clone(), &node.name)
        .await?;
    let changed = match &existing {
        Some(e) => e.content_hash != node.content_hash,
        None => true,
    };

    let node_id = store.upsert_node(node).await?;

    if changed {
        let invalidated = store.invalidate_analyses(node_id).await?;
        if invalidated > 0 {
            debug!(
                node_id = node_id.0,
                name = %node.name,
                invalidated,
                "Invalidated stale analyses after content change"
            );
        }
    }

    Ok((node_id, changed))
}

/// Cascade invalidation: invalidate analysis results for all nodes connected
/// to the given node via hyperedges.
///
/// This ensures that when a file changes, analyses that depend on its neighbors
/// (e.g., co-change, call graph centrality) are also marked stale.
pub async fn invalidate_dependents(
    store: &dyn HomerStore,
    node_id: NodeId,
) -> crate::error::Result<u64> {
    let edges = store.get_edges_involving(node_id).await?;

    let mut affected: HashSet<NodeId> = HashSet::new();
    for edge in &edges {
        for member in &edge.members {
            if member.node_id != node_id {
                affected.insert(member.node_id);
            }
        }
    }

    let mut total_invalidated = 0u64;
    for dep_id in &affected {
        let count = store.invalidate_analyses(*dep_id).await?;
        total_invalidated += count;
    }

    if total_invalidated > 0 {
        debug!(
            source = node_id.0,
            dependents = affected.len(),
            invalidated = total_invalidated,
            "Cascaded invalidation to dependents"
        );
    }

    Ok(total_invalidated)
}

/// Check if an extractor needs to run by comparing its checkpoint against current state.
///
/// Returns `true` if the extractor should run (checkpoint doesn't exist or differs).
pub async fn needs_extraction(
    store: &dyn HomerStore,
    checkpoint_key: &str,
    current_state: &str,
) -> crate::error::Result<bool> {
    let stored = store.get_checkpoint(checkpoint_key).await?;
    Ok(stored.as_deref() != Some(current_state))
}

/// Compute a content hash for a byte slice using a fast non-cryptographic hash.
///
/// Uses FNV-1a for speed; collisions are acceptable since this is for change detection only.
pub fn content_hash(data: &[u8]) -> u64 {
    // FNV-1a 64-bit
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{AnalysisKind, AnalysisResult, AnalysisResultId, NodeKind};
    use chrono::Utc;
    use std::collections::HashMap;

    #[tokio::test]
    async fn upsert_if_changed_invalidates_on_hash_change() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        // Create initial node with hash
        let node_v1 = Node {
            id: NodeId(0),
            kind: NodeKind::File,
            name: "src/main.rs".to_string(),
            content_hash: Some(111),
            last_extracted: now,
            metadata: HashMap::new(),
        };

        let (id, changed) = upsert_if_changed(&store, &node_v1).await.unwrap();
        assert!(changed, "First insert should be 'changed'");

        // Store an analysis result for this node
        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: id,
                kind: AnalysisKind::ChangeFrequency,
                data: serde_json::json!({"total": 5}),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        // Upsert same hash — should NOT invalidate
        let (_, changed) = upsert_if_changed(&store, &node_v1).await.unwrap();
        assert!(!changed, "Same hash should not be 'changed'");

        let analysis = store
            .get_analysis(id, AnalysisKind::ChangeFrequency)
            .await
            .unwrap();
        assert!(analysis.is_some(), "Analysis should still exist");

        // Upsert different hash — SHOULD invalidate
        let node_v2 = Node {
            content_hash: Some(222),
            ..node_v1.clone()
        };
        let (_, changed) = upsert_if_changed(&store, &node_v2).await.unwrap();
        assert!(changed, "Different hash should be 'changed'");

        let analysis = store
            .get_analysis(id, AnalysisKind::ChangeFrequency)
            .await
            .unwrap();
        assert!(analysis.is_none(), "Analysis should be invalidated");
    }

    #[tokio::test]
    async fn needs_extraction_detects_state_change() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(
            needs_extraction(&store, "git_last_sha", "abc123")
                .await
                .unwrap(),
            "Should need extraction when no checkpoint exists"
        );

        store
            .set_checkpoint("git_last_sha", "abc123")
            .await
            .unwrap();

        assert!(
            !needs_extraction(&store, "git_last_sha", "abc123")
                .await
                .unwrap(),
            "Should NOT need extraction when checkpoint matches"
        );

        assert!(
            needs_extraction(&store, "git_last_sha", "def456")
                .await
                .unwrap(),
            "Should need extraction when checkpoint differs"
        );
    }

    #[test]
    fn content_hash_deterministic() {
        let data = b"fn main() { println!(\"hello\"); }";
        let h1 = content_hash(data);
        let h2 = content_hash(data);
        assert_eq!(h1, h2, "Same data should produce same hash");

        let h3 = content_hash(b"different content");
        assert_ne!(h1, h3, "Different data should produce different hash");
    }

    #[test]
    fn content_hash_empty() {
        let h = content_hash(b"");
        assert_ne!(h, 0, "Empty data should still produce a non-zero hash");
    }
}
