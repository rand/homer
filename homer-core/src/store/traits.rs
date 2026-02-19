use chrono::{DateTime, Utc};

use crate::types::{
    AnalysisKind, AnalysisResult, GraphDiff, Hyperedge, HyperedgeId, HyperedgeKind, InMemoryGraph,
    Node, NodeFilter, NodeId, NodeKind, SearchHit, SearchScope, SnapshotId, SnapshotInfo,
    StoreStats, SubgraphFilter,
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

    /// Delete nodes marked stale before `older_than`. Returns count deleted.
    async fn delete_stale_nodes(&self, older_than: DateTime<Utc>) -> crate::error::Result<u64>;

    /// Batch upsert nodes within a single transaction. Returns IDs.
    async fn upsert_nodes_batch(&self, nodes: &[Node]) -> crate::error::Result<Vec<NodeId>>;

    // ── Entity aliasing ──────────────────────────────────────────

    /// Resolve a node to its canonical (current) identity through alias chains.
    async fn resolve_canonical(&self, node_id: NodeId) -> crate::error::Result<NodeId>;

    /// Return the full alias chain starting from `node_id`, following old → new links.
    /// The first element is `node_id` itself; the last is the canonical identity.
    /// Returns `[node_id]` if no aliases exist.
    async fn alias_chain(&self, node_id: NodeId) -> crate::error::Result<Vec<NodeId>>;

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

    /// Invalidate analysis results of specific kinds for a node.
    async fn invalidate_analyses_by_kinds(
        &self,
        node_id: NodeId,
        kinds: &[AnalysisKind],
    ) -> crate::error::Result<u64>;

    /// Invalidate analysis results of specific kinds for ALL nodes.
    async fn invalidate_all_by_kinds(&self, kinds: &[AnalysisKind]) -> crate::error::Result<u64>;

    /// Invalidate all analysis results for a node EXCEPT the specified kinds.
    async fn invalidate_analyses_excluding_kinds(
        &self,
        node_id: NodeId,
        keep_kinds: &[AnalysisKind],
    ) -> crate::error::Result<u64>;

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

    /// Clear analysis results for specific kinds (for --force-semantic).
    async fn clear_analyses_by_kinds(&self, kinds: &[AnalysisKind]) -> crate::error::Result<()>;

    // ── Graph snapshots ────────────────────────────────────────────

    /// Create a named snapshot of current graph state.
    async fn create_snapshot(&self, label: &str) -> crate::error::Result<SnapshotId>;

    /// List all snapshots ordered by creation time.
    async fn list_snapshots(&self) -> crate::error::Result<Vec<SnapshotInfo>>;

    /// Delete a snapshot by its label.
    async fn delete_snapshot(&self, label: &str) -> crate::error::Result<bool>;

    /// Compute the diff between two snapshots (added/removed nodes and edges).
    async fn get_snapshot_diff(
        &self,
        from: SnapshotId,
        to: SnapshotId,
    ) -> crate::error::Result<GraphDiff>;

    // ── Graph loading ────────────────────────────────────────────────

    /// Load the call graph (`Calls` edges) into memory, applying a subgraph filter.
    async fn load_call_graph(&self, filter: &SubgraphFilter)
    -> crate::error::Result<InMemoryGraph>;

    /// Load the import graph (`Imports` edges) into memory, applying a subgraph filter.
    async fn load_import_graph(
        &self,
        filter: &SubgraphFilter,
    ) -> crate::error::Result<InMemoryGraph>;

    // ── Transactions ──────────────────────────────────────────────

    /// Begin an explicit transaction. Operations between begin and commit
    /// are executed atomically. Default: no-op (each operation auto-commits).
    async fn begin_transaction(&self) -> crate::error::Result<()> {
        Ok(())
    }

    /// Commit the current transaction started by `begin_transaction`.
    async fn commit_transaction(&self) -> crate::error::Result<()> {
        Ok(())
    }

    /// Roll back the current transaction started by `begin_transaction`.
    async fn rollback_transaction(&self) -> crate::error::Result<()> {
        Ok(())
    }

    // ── Metrics ────────────────────────────────────────────────────

    /// Get summary statistics about the store.
    async fn stats(&self) -> crate::error::Result<StoreStats>;
}
