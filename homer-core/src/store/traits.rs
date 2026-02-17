use crate::types::{
    AnalysisKind, AnalysisResult, Hyperedge, HyperedgeId, HyperedgeKind, Node, NodeFilter, NodeId,
    NodeKind, SearchHit, SearchScope, SnapshotId, StoreStats,
};

/// The core store abstraction. All pipeline stages read/write through this trait.
#[async_trait::async_trait]
pub trait HomerStore: Send + Sync {
    // ── Node operations ────────────────────────────────────────────

    /// Insert or update a node. Returns the node's ID.
    async fn upsert_node(&self, node: &Node) -> crate::error::Result<NodeId>;

    /// Get a node by its ID.
    async fn get_node(&self, id: NodeId) -> crate::error::Result<Option<Node>>;

    /// Get a node by its kind and canonical name.
    async fn get_node_by_name(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> crate::error::Result<Option<Node>>;

    /// Find nodes matching a filter.
    async fn find_nodes(&self, filter: &NodeFilter) -> crate::error::Result<Vec<Node>>;

    /// Mark a node as stale (soft delete).
    async fn mark_node_stale(&self, id: NodeId) -> crate::error::Result<()>;

    // ── Hyperedge operations ───────────────────────────────────────

    /// Insert or update a hyperedge. Returns the edge's ID.
    async fn upsert_hyperedge(&self, edge: &Hyperedge) -> crate::error::Result<HyperedgeId>;

    /// Get all edges involving a specific node.
    async fn get_edges_involving(&self, node_id: NodeId) -> crate::error::Result<Vec<Hyperedge>>;

    /// Get all edges of a specific kind.
    async fn get_edges_by_kind(&self, kind: HyperedgeKind) -> crate::error::Result<Vec<Hyperedge>>;

    /// Get all co-member node IDs for a given node in edges of a specific kind.
    async fn get_co_members(
        &self,
        node_id: NodeId,
        edge_kind: HyperedgeKind,
    ) -> crate::error::Result<Vec<NodeId>>;

    // ── Analysis results ───────────────────────────────────────────

    /// Store an analysis result (upsert by `node_id` + kind).
    async fn store_analysis(&self, result: &AnalysisResult) -> crate::error::Result<()>;

    /// Get analysis result for a specific node and analysis kind.
    async fn get_analysis(
        &self,
        node_id: NodeId,
        kind: AnalysisKind,
    ) -> crate::error::Result<Option<AnalysisResult>>;

    /// Get all analysis results of a specific kind.
    async fn get_analyses_by_kind(
        &self,
        kind: AnalysisKind,
    ) -> crate::error::Result<Vec<AnalysisResult>>;

    /// Invalidate all analysis results for a node.
    async fn invalidate_analyses(&self, node_id: NodeId) -> crate::error::Result<u64>;

    // ── Full-text search ───────────────────────────────────────────

    /// Index text content for a node.
    async fn index_text(
        &self,
        node_id: NodeId,
        content_type: &str,
        content: &str,
    ) -> crate::error::Result<()>;

    /// Search indexed text content.
    async fn search_text(
        &self,
        query: &str,
        scope: SearchScope,
    ) -> crate::error::Result<Vec<SearchHit>>;

    // ── Checkpoints ────────────────────────────────────────────────

    /// Get a checkpoint value.
    async fn get_checkpoint(&self, kind: &str) -> crate::error::Result<Option<String>>;

    /// Set a checkpoint value.
    async fn set_checkpoint(&self, kind: &str, value: &str) -> crate::error::Result<()>;

    /// Clear all checkpoints (for --force re-extraction).
    async fn clear_checkpoints(&self) -> crate::error::Result<()>;

    /// Clear all analysis results (for --force-analysis).
    async fn clear_analyses(&self) -> crate::error::Result<()>;

    // ── Graph snapshots ────────────────────────────────────────────

    /// Create a named snapshot of current graph state.
    async fn create_snapshot(&self, label: &str) -> crate::error::Result<SnapshotId>;

    // ── Metrics ────────────────────────────────────────────────────

    /// Get summary statistics about the store.
    async fn stats(&self) -> crate::error::Result<StoreStats>;
}
