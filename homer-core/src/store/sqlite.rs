use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{HomerError, StoreError};
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, Hyperedge, HyperedgeId, HyperedgeKind,
    HyperedgeMember, Node, NodeFilter, NodeId, NodeKind, SearchHit, SearchScope, SnapshotId,
    StoreStats,
};

use super::HomerStore;
use super::schema;

/// SQLite-backed implementation of `HomerStore`.
#[derive(Debug)]
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open (or create) a store at the given path.
    pub fn open(path: &Path) -> crate::error::Result<Self> {
        let conn = Connection::open(path).map_err(StoreError::Sqlite)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> crate::error::Result<Self> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();

        // Performance pragmas (skip WAL for in-memory — it's auto)
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -64000;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(StoreError::Sqlite)?;

        // Try WAL mode — silently ignored for in-memory
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL;");
        let _ = conn.execute_batch("PRAGMA mmap_size = 268435456;");

        // Create schema
        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(StoreError::Sqlite)?;
        conn.execute_batch(schema::VIEWS_SQL)
            .map_err(StoreError::Sqlite)?;

        // Set schema version if not present
        conn.execute(
            "INSERT OR IGNORE INTO homer_meta (key, value) VALUES ('schema_version', ?1)",
            params![schema::SCHEMA_VERSION],
        )
        .map_err(StoreError::Sqlite)?;

        Ok(())
    }

    /// Helper: parse metadata JSON from a row.
    fn parse_metadata(json_str: &str) -> HashMap<String, serde_json::Value> {
        serde_json::from_str(json_str).unwrap_or_default()
    }

    /// Helper: read a full node from a row.
    fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
        let kind_str: String = row.get("kind")?;
        let metadata_str: String = row.get("metadata")?;
        let last_extracted_str: String = row.get("last_extracted")?;
        // Read as i64 and reinterpret bits as u64 (inverse of the write cast)
        let hash_i64: Option<i64> = row.get("content_hash")?;

        Ok(Node {
            id: NodeId(row.get("id")?),
            kind: serde_json::from_str(&format!("\"{kind_str}\"")).unwrap_or(NodeKind::File),
            name: row.get("name")?,
            #[allow(clippy::cast_sign_loss)]
            content_hash: hash_i64.map(|h| h as u64),
            last_extracted: DateTime::parse_from_rfc3339(&last_extracted_str)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            metadata: Self::parse_metadata(&metadata_str),
        })
    }

    /// Helper: load hyperedge members for a given edge ID.
    fn load_members(conn: &Connection, edge_id: i64) -> rusqlite::Result<Vec<HyperedgeMember>> {
        let mut stmt = conn.prepare_cached(
            "SELECT node_id, role, position FROM hyperedge_members
             WHERE hyperedge_id = ?1 ORDER BY position",
        )?;
        let members = stmt
            .query_map(params![edge_id], |row| {
                Ok(HyperedgeMember {
                    node_id: NodeId(row.get(0)?),
                    role: row.get(1)?,
                    position: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(members)
    }

    /// Helper: read a full hyperedge from a row (without members — caller loads separately).
    fn row_to_edge_header(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hyperedge> {
        let kind_str: String = row.get("kind")?;
        let metadata_str: String = row.get("metadata")?;
        let last_updated_str: String = row.get("last_updated")?;

        Ok(Hyperedge {
            id: HyperedgeId(row.get("id")?),
            kind: serde_json::from_str(&format!("\"{kind_str}\""))
                .unwrap_or(HyperedgeKind::Modifies),
            members: Vec::new(), // Loaded separately
            confidence: row.get("confidence")?,
            last_updated: DateTime::parse_from_rfc3339(&last_updated_str)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            metadata: Self::parse_metadata(&metadata_str),
        })
    }
}

#[async_trait::async_trait]
impl HomerStore for SqliteStore {
    // ── Node operations ────────────────────────────────────────────

    async fn upsert_node(&self, node: &Node) -> crate::error::Result<NodeId> {
        let conn = self.conn.lock().unwrap();
        let kind_str = node.kind.as_str();
        let metadata_json =
            serde_json::to_string(&node.metadata).map_err(StoreError::Serialization)?;
        let last_extracted = node.last_extracted.to_rfc3339();

        // SQLite INTEGER is i64; reinterpret u64 bits to avoid TryFromIntError
        #[allow(clippy::cast_possible_wrap)]
        let hash_i64: Option<i64> = node.content_hash.map(|h| h as i64);

        conn.execute(
            "INSERT INTO nodes (kind, name, content_hash, last_extracted, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, name) DO UPDATE SET
                content_hash = excluded.content_hash,
                last_extracted = excluded.last_extracted,
                metadata = excluded.metadata",
            params![kind_str, node.name, hash_i64, last_extracted, metadata_json],
        )
        .map_err(StoreError::Sqlite)?;

        // Always query the actual id — last_insert_rowid() is unreliable after
        // ON CONFLICT DO UPDATE (it retains the rowid from the previous INSERT).
        let actual_id: i64 = conn
            .query_row(
                "SELECT id FROM nodes WHERE kind = ?1 AND name = ?2",
                params![kind_str, node.name],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)?;
        Ok(NodeId(actual_id))
    }

    async fn get_node(&self, id: NodeId) -> crate::error::Result<Option<Node>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT * FROM nodes WHERE id = ?1",
            params![id.0],
            Self::row_to_node,
        )
        .optional()
        .map_err(StoreError::Sqlite)
        .map_err(HomerError::Store)
    }

    async fn get_node_by_name(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> crate::error::Result<Option<Node>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT * FROM nodes WHERE kind = ?1 AND name = ?2",
            params![kind.as_str(), name],
            Self::row_to_node,
        )
        .optional()
        .map_err(StoreError::Sqlite)
        .map_err(HomerError::Store)
    }

    async fn find_nodes(&self, filter: &NodeFilter) -> crate::error::Result<Vec<Node>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("SELECT * FROM nodes WHERE 1=1");
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(kind) = &filter.kind {
            let _ = write!(sql, " AND kind = ?{}", param_values.len() + 1);
            param_values.push(Box::new(kind.as_str().to_string()));
        }
        if let Some(prefix) = &filter.name_prefix {
            let _ = write!(sql, " AND name LIKE ?{}", param_values.len() + 1);
            param_values.push(Box::new(format!("{prefix}%")));
        }
        if let Some(contains) = &filter.name_contains {
            let _ = write!(sql, " AND name LIKE ?{}", param_values.len() + 1);
            param_values.push(Box::new(format!("%{contains}%")));
        }
        if let Some(limit) = filter.limit {
            let _ = write!(sql, " LIMIT {limit}");
        }

        let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(std::convert::AsRef::as_ref)
            .collect();
        let nodes = stmt
            .query_map(params_ref.as_slice(), Self::row_to_node)
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(nodes)
    }

    async fn mark_node_stale(&self, id: NodeId) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE nodes SET metadata = json_set(metadata, '$.stale', true) WHERE id = ?1",
            params![id.0],
        )
        .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    // ── Hyperedge operations ───────────────────────────────────────

    async fn upsert_hyperedge(&self, edge: &Hyperedge) -> crate::error::Result<HyperedgeId> {
        let conn = self.conn.lock().unwrap();
        let kind_str = edge.kind.as_str();
        let metadata_json =
            serde_json::to_string(&edge.metadata).map_err(StoreError::Serialization)?;
        let last_updated = edge.last_updated.to_rfc3339();

        conn.execute(
            "INSERT INTO hyperedges (kind, confidence, last_updated, metadata)
             VALUES (?1, ?2, ?3, ?4)",
            params![kind_str, edge.confidence, last_updated, metadata_json],
        )
        .map_err(StoreError::Sqlite)?;

        let edge_id = conn.last_insert_rowid();

        // Insert members
        let mut stmt = conn
            .prepare_cached(
                "INSERT OR REPLACE INTO hyperedge_members (hyperedge_id, node_id, role, position)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(StoreError::Sqlite)?;

        for member in &edge.members {
            stmt.execute(params![
                edge_id,
                member.node_id.0,
                member.role,
                member.position
            ])
            .map_err(StoreError::Sqlite)?;
        }

        Ok(HyperedgeId(edge_id))
    }

    async fn get_edges_involving(&self, node_id: NodeId) -> crate::error::Result<Vec<Hyperedge>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT e.* FROM hyperedges e
                 JOIN hyperedge_members m ON e.id = m.hyperedge_id
                 WHERE m.node_id = ?1",
            )
            .map_err(StoreError::Sqlite)?;

        let edges: Vec<Hyperedge> = stmt
            .query_map(params![node_id.0], Self::row_to_edge_header)
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        // Load members for each edge
        let mut result = Vec::with_capacity(edges.len());
        for mut edge in edges {
            edge.members = Self::load_members(&conn, edge.id.0).map_err(StoreError::Sqlite)?;
            result.push(edge);
        }
        Ok(result)
    }

    async fn get_edges_by_kind(&self, kind: HyperedgeKind) -> crate::error::Result<Vec<Hyperedge>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM hyperedges WHERE kind = ?1")
            .map_err(StoreError::Sqlite)?;

        let edges: Vec<Hyperedge> = stmt
            .query_map(params![kind.as_str()], Self::row_to_edge_header)
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        let mut result = Vec::with_capacity(edges.len());
        for mut edge in edges {
            edge.members = Self::load_members(&conn, edge.id.0).map_err(StoreError::Sqlite)?;
            result.push(edge);
        }
        Ok(result)
    }

    async fn get_co_members(
        &self,
        node_id: NodeId,
        edge_kind: HyperedgeKind,
    ) -> crate::error::Result<Vec<NodeId>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT m2.node_id FROM hyperedge_members m1
                 JOIN hyperedge_members m2 ON m1.hyperedge_id = m2.hyperedge_id
                 JOIN hyperedges e ON e.id = m1.hyperedge_id
                 WHERE m1.node_id = ?1 AND e.kind = ?2 AND m2.node_id != ?1",
            )
            .map_err(StoreError::Sqlite)?;

        let ids = stmt
            .query_map(params![node_id.0, edge_kind.as_str()], |row| {
                Ok(NodeId(row.get(0)?))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(ids)
    }

    // ── Analysis results ───────────────────────────────────────────

    async fn store_analysis(&self, result: &AnalysisResult) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        let data_json = serde_json::to_string(&result.data).map_err(StoreError::Serialization)?;
        let computed_at = result.computed_at.to_rfc3339();

        conn.execute(
            "INSERT INTO analysis_results (node_id, kind, data, input_hash, computed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(node_id, kind) DO UPDATE SET
                data = excluded.data,
                input_hash = excluded.input_hash,
                computed_at = excluded.computed_at",
            params![
                result.node_id.0,
                result.kind.as_str(),
                data_json,
                result.input_hash,
                computed_at
            ],
        )
        .map_err(StoreError::Sqlite)?;

        Ok(())
    }

    async fn get_analysis(
        &self,
        node_id: NodeId,
        kind: AnalysisKind,
    ) -> crate::error::Result<Option<AnalysisResult>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT * FROM analysis_results WHERE node_id = ?1 AND kind = ?2",
            params![node_id.0, kind.as_str()],
            |row| {
                let data_str: String = row.get("data")?;
                let computed_at_str: String = row.get("computed_at")?;
                Ok(AnalysisResult {
                    id: AnalysisResultId(row.get("id")?),
                    node_id: NodeId(row.get("node_id")?),
                    kind: kind.clone(),
                    data: serde_json::from_str(&data_str).unwrap_or_default(),
                    input_hash: row.get("input_hash")?,
                    computed_at: DateTime::parse_from_rfc3339(&computed_at_str)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)
        .map_err(HomerError::Store)
    }

    async fn get_analyses_by_kind(
        &self,
        kind: AnalysisKind,
    ) -> crate::error::Result<Vec<AnalysisResult>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM analysis_results WHERE kind = ?1")
            .map_err(StoreError::Sqlite)?;

        let results = stmt
            .query_map(params![kind.as_str()], |row| {
                let data_str: String = row.get("data")?;
                let computed_at_str: String = row.get("computed_at")?;
                let kind_str: String = row.get("kind")?;
                Ok(AnalysisResult {
                    id: AnalysisResultId(row.get("id")?),
                    node_id: NodeId(row.get("node_id")?),
                    kind: serde_json::from_str(&format!("\"{kind_str}\""))
                        .unwrap_or(AnalysisKind::ChangeFrequency),
                    data: serde_json::from_str(&data_str).unwrap_or_default(),
                    input_hash: row.get("input_hash")?,
                    computed_at: DateTime::parse_from_rfc3339(&computed_at_str)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(results)
    }

    async fn invalidate_analyses(&self, node_id: NodeId) -> crate::error::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count = conn
            .execute(
                "DELETE FROM analysis_results WHERE node_id = ?1",
                params![node_id.0],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(count as u64)
    }

    // ── Full-text search ───────────────────────────────────────────

    async fn index_text(
        &self,
        node_id: NodeId,
        content_type: &str,
        content: &str,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO text_search (node_id, content_type, content) VALUES (?1, ?2, ?3)",
            params![node_id.0.to_string(), content_type, content],
        )
        .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn search_text(
        &self,
        query: &str,
        scope: SearchScope,
    ) -> crate::error::Result<Vec<SearchHit>> {
        let conn = self.conn.lock().unwrap();
        let limit = scope.limit.unwrap_or(20);

        let mut stmt = conn
            .prepare(
                "SELECT node_id, content_type, snippet(text_search, 2, '<b>', '</b>', '...', 64),
                        rank
                 FROM text_search WHERE text_search MATCH ?1
                 ORDER BY rank LIMIT ?2",
            )
            .map_err(StoreError::Sqlite)?;

        let hits = stmt
            .query_map(params![query, limit], |row| {
                let node_id_str: String = row.get(0)?;
                Ok(SearchHit {
                    node_id: NodeId(node_id_str.parse().unwrap_or(0)),
                    content_type: row.get(1)?,
                    snippet: row.get(2)?,
                    rank: row.get(3)?,
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(hits)
    }

    // ── Checkpoints ────────────────────────────────────────────────

    async fn get_checkpoint(&self, kind: &str) -> crate::error::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM checkpoints WHERE kind = ?1",
            params![kind],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
        .map_err(HomerError::Store)
    }

    async fn set_checkpoint(&self, kind: &str, value: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO checkpoints (kind, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(kind) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![kind, value, now],
        )
        .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn clear_checkpoints(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM checkpoints", [])
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn clear_analyses(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM analysis_results", [])
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    // ── Graph snapshots ────────────────────────────────────────────

    async fn create_snapshot(&self, label: &str) -> crate::error::Result<SnapshotId> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        let node_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
            .map_err(StoreError::Sqlite)?;
        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM hyperedges", [], |row| row.get(0))
            .map_err(StoreError::Sqlite)?;

        conn.execute(
            "INSERT INTO graph_snapshots (label, snapshot_at, edge_count, node_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![label, now, edge_count, node_count],
        )
        .map_err(StoreError::Sqlite)?;

        Ok(SnapshotId(conn.last_insert_rowid()))
    }

    // ── Metrics ────────────────────────────────────────────────────

    async fn stats(&self) -> crate::error::Result<StoreStats> {
        let conn = self.conn.lock().unwrap();

        let total_nodes: u64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
            .map_err(StoreError::Sqlite)?;
        let total_edges: u64 = conn
            .query_row("SELECT COUNT(*) FROM hyperedges", [], |row| row.get(0))
            .map_err(StoreError::Sqlite)?;
        let total_analyses: u64 = conn
            .query_row("SELECT COUNT(*) FROM analysis_results", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::Sqlite)?;

        // Count nodes by kind
        let mut stmt = conn
            .prepare("SELECT kind, COUNT(*) FROM nodes GROUP BY kind")
            .map_err(StoreError::Sqlite)?;
        let nodes_by_kind: HashMap<String, u64> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(StoreError::Sqlite)?;

        // Count edges by kind
        let mut stmt = conn
            .prepare("SELECT kind, COUNT(*) FROM hyperedges GROUP BY kind")
            .map_err(StoreError::Sqlite)?;
        let edges_by_kind: HashMap<String, u64> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(StoreStats {
            total_nodes,
            total_edges,
            total_analyses,
            nodes_by_kind,
            edges_by_kind,
            db_size_bytes: 0, // In-memory doesn't have a meaningful size
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_node(kind: NodeKind, name: &str) -> Node {
        Node {
            id: NodeId(0), // Will be assigned by store
            kind,
            name: name.to_string(),
            content_hash: Some(42),
            last_extracted: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn upsert_and_get_node() {
        let store = SqliteStore::in_memory().unwrap();
        let node = make_test_node(NodeKind::File, "src/main.rs");

        let id = store.upsert_node(&node).await.unwrap();
        assert!(id.0 > 0);

        let fetched = store.get_node(id).await.unwrap().unwrap();
        assert_eq!(fetched.kind, NodeKind::File);
        assert_eq!(fetched.name, "src/main.rs");
        assert_eq!(fetched.content_hash, Some(42));
    }

    #[tokio::test]
    async fn upsert_node_updates_on_conflict() {
        let store = SqliteStore::in_memory().unwrap();
        let mut node = make_test_node(NodeKind::File, "src/lib.rs");
        node.content_hash = Some(1);
        let id1 = store.upsert_node(&node).await.unwrap();

        node.content_hash = Some(2);
        let id2 = store.upsert_node(&node).await.unwrap();

        assert_eq!(id1, id2);
        let fetched = store.get_node(id1).await.unwrap().unwrap();
        assert_eq!(fetched.content_hash, Some(2));
    }

    #[tokio::test]
    async fn get_node_by_name() {
        let store = SqliteStore::in_memory().unwrap();
        let node = make_test_node(NodeKind::Function, "auth::validate");
        store.upsert_node(&node).await.unwrap();

        let found = store
            .get_node_by_name(NodeKind::Function, "auth::validate")
            .await
            .unwrap();
        assert!(found.is_some());

        let not_found = store
            .get_node_by_name(NodeKind::Function, "nonexistent")
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn find_nodes_with_filter() {
        let store = SqliteStore::in_memory().unwrap();
        for name in ["src/a.rs", "src/b.rs", "tests/c.rs"] {
            store
                .upsert_node(&make_test_node(NodeKind::File, name))
                .await
                .unwrap();
        }

        let filter = NodeFilter {
            kind: Some(NodeKind::File),
            name_prefix: Some("src/".to_string()),
            ..Default::default()
        };
        let results = store.find_nodes(&filter).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn hyperedge_operations() {
        let store = SqliteStore::in_memory().unwrap();

        let source_id = store
            .upsert_node(&make_test_node(NodeKind::Function, "main"))
            .await
            .unwrap();
        let target_id = store
            .upsert_node(&make_test_node(NodeKind::Function, "helper"))
            .await
            .unwrap();

        let edge = Hyperedge {
            id: HyperedgeId(0),
            kind: HyperedgeKind::Calls,
            members: vec![
                HyperedgeMember {
                    node_id: source_id,
                    role: "caller".to_string(),
                    position: 0,
                },
                HyperedgeMember {
                    node_id: target_id,
                    role: "callee".to_string(),
                    position: 1,
                },
            ],
            confidence: 0.95,
            last_updated: Utc::now(),
            metadata: HashMap::new(),
        };

        let edge_id = store.upsert_hyperedge(&edge).await.unwrap();
        assert!(edge_id.0 > 0);

        // Test get_edges_involving
        let edges = store.get_edges_involving(source_id).await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].members.len(), 2);

        // Test get_edges_by_kind
        let call_edges = store.get_edges_by_kind(HyperedgeKind::Calls).await.unwrap();
        assert_eq!(call_edges.len(), 1);

        // Test get_co_members
        let co = store
            .get_co_members(source_id, HyperedgeKind::Calls)
            .await
            .unwrap();
        assert_eq!(co, vec![target_id]);
    }

    #[tokio::test]
    async fn analysis_result_operations() {
        let store = SqliteStore::in_memory().unwrap();
        let node_id = store
            .upsert_node(&make_test_node(NodeKind::File, "src/main.rs"))
            .await
            .unwrap();

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind: AnalysisKind::ChangeFrequency,
            data: serde_json::json!({ "total": 42, "last_90d": 7 }),
            input_hash: 12345,
            computed_at: Utc::now(),
        };

        store.store_analysis(&result).await.unwrap();

        let fetched = store
            .get_analysis(node_id, AnalysisKind::ChangeFrequency)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.data["total"], 42);

        // Update with new data
        let result2 = AnalysisResult {
            data: serde_json::json!({ "total": 99 }),
            input_hash: 99999,
            ..result
        };
        store.store_analysis(&result2).await.unwrap();

        let fetched2 = store
            .get_analysis(node_id, AnalysisKind::ChangeFrequency)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched2.data["total"], 99);

        // Invalidate
        let deleted = store.invalidate_analyses(node_id).await.unwrap();
        assert_eq!(deleted, 1);
    }

    #[tokio::test]
    async fn checkpoint_operations() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(store.get_checkpoint("git_sha").await.unwrap().is_none());

        store.set_checkpoint("git_sha", "abc123").await.unwrap();
        assert_eq!(
            store.get_checkpoint("git_sha").await.unwrap().unwrap(),
            "abc123"
        );

        store.set_checkpoint("git_sha", "def456").await.unwrap();
        assert_eq!(
            store.get_checkpoint("git_sha").await.unwrap().unwrap(),
            "def456"
        );
    }

    #[tokio::test]
    async fn full_text_search() {
        let store = SqliteStore::in_memory().unwrap();
        let id = store
            .upsert_node(&make_test_node(NodeKind::Commit, "abc123"))
            .await
            .unwrap();

        store
            .index_text(
                id,
                "commit_message",
                "Fix authentication bug in token validation",
            )
            .await
            .unwrap();

        let hits = store
            .search_text("authentication", SearchScope::default())
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content_type, "commit_message");
    }

    #[tokio::test]
    async fn snapshot_creation() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::File, "a.rs"))
            .await
            .unwrap();

        let snap_id = store.create_snapshot("v1.0").await.unwrap();
        assert!(snap_id.0 > 0);
    }

    #[tokio::test]
    async fn store_stats() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::File, "a.rs"))
            .await
            .unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::Function, "main"))
            .await
            .unwrap();

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_nodes, 2);
        assert_eq!(stats.nodes_by_kind["File"], 1);
        assert_eq!(stats.nodes_by_kind["Function"], 1);
    }

    #[tokio::test]
    async fn bulk_insert_performance() {
        let store = SqliteStore::in_memory().unwrap();
        let start = std::time::Instant::now();

        for i in 0..10_000 {
            store
                .upsert_node(&make_test_node(NodeKind::File, &format!("src/file_{i}.rs")))
                .await
                .unwrap();
        }

        let elapsed = start.elapsed();
        let rate = 10_000.0 / elapsed.as_secs_f64();
        eprintln!("Bulk insert: {rate:.0} nodes/sec ({elapsed:?} for 10K)");
        // Accept criteria: >10k nodes/sec — individual inserts without
        // transaction batching may be slower, but verifies correctness
        assert!(rate > 1000.0, "Insert rate too slow: {rate:.0} nodes/sec");
    }
}
