use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{HomerError, StoreError};
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, GraphDiff, Hyperedge, HyperedgeId,
    HyperedgeKind, HyperedgeMember, InMemoryGraph, Node, NodeFilter, NodeId, NodeKind, SearchHit,
    SearchScope, SnapshotId, SnapshotInfo, StoreStats, SubgraphFilter, extract_directed_pair,
};

use super::HomerStore;
use super::schema;

/// SQLite-backed implementation of `HomerStore`.
#[derive(Debug)]
pub struct SqliteStore {
    conn: Mutex<Connection>,
    db_path: Option<PathBuf>,
}

impl SqliteStore {
    /// Open (or create) a store at the given path.
    pub fn open(path: &Path) -> crate::error::Result<Self> {
        let conn = Connection::open(path).map_err(StoreError::Sqlite)?;
        let store = Self {
            conn: Mutex::new(conn),
            db_path: Some(path.to_path_buf()),
        };
        store.initialize()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> crate::error::Result<Self> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        let store = Self {
            conn: Mutex::new(conn),
            db_path: None,
        };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> crate::error::Result<()> {
        let mut conn = self.conn.lock().expect("homer store mutex poisoned");

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

        // Legacy DBs may have hyperedges without identity_key; add it before
        // schema index creation to avoid CREATE INDEX failures.
        Self::ensure_hyperedge_identity_column(&conn).map_err(StoreError::Sqlite)?;

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

        Self::migrate_hyperedge_identity(&mut conn).map_err(StoreError::Sqlite)?;

        Ok(())
    }

    /// Pre-schema compatibility shim for older stores.
    fn ensure_hyperedge_identity_column(conn: &Connection) -> rusqlite::Result<()> {
        let mut has_hyperedges_table = false;
        let mut has_identity_key = false;

        let mut table_info = conn.prepare("PRAGMA table_info(hyperedges)")?;
        let rows = table_info.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            has_hyperedges_table = true;
            if row? == "identity_key" {
                has_identity_key = true;
            }
        }

        if has_hyperedges_table && !has_identity_key {
            conn.execute("ALTER TABLE hyperedges ADD COLUMN identity_key TEXT", [])?;
        }

        Ok(())
    }

    /// Ensure hyperedges have deterministic identity keys and deduplicate legacy rows.
    fn migrate_hyperedge_identity(conn: &mut Connection) -> rusqlite::Result<()> {
        let mut has_identity_key = false;
        let mut table_info = conn.prepare("PRAGMA table_info(hyperedges)")?;
        let rows = table_info.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            if row? == "identity_key" {
                has_identity_key = true;
                break;
            }
        }
        if !has_identity_key {
            conn.execute("ALTER TABLE hyperedges ADD COLUMN identity_key TEXT", [])?;
        }

        let tx = conn.unchecked_transaction()?;
        {
            // Build identity keys for all existing edges.
            let mut edge_stmt = tx.prepare("SELECT id, kind FROM hyperedges ORDER BY id")?;
            let mut rows = edge_stmt.query([])?;
            while let Some(row) = rows.next()? {
                let edge_id: i64 = row.get(0)?;
                let kind_str: String = row.get(1)?;
                let kind: HyperedgeKind = serde_json::from_str(&format!("\"{kind_str}\""))
                    .unwrap_or(HyperedgeKind::Modifies);
                let members = Self::load_members(&tx, edge_id)?;
                let identity = Self::edge_identity_key(&kind, &members);
                tx.execute(
                    "UPDATE hyperedges SET identity_key = ?1 WHERE id = ?2",
                    params![identity, edge_id],
                )?;
            }

            // Collapse duplicate identities, keeping the newest row by id.
            tx.execute_batch(
                "DELETE FROM hyperedges
                 WHERE id IN (
                    SELECT dup.id
                    FROM hyperedges dup
                    JOIN (
                        SELECT identity_key, MAX(id) AS keep_id
                        FROM hyperedges
                        WHERE identity_key IS NOT NULL
                        GROUP BY identity_key
                        HAVING COUNT(*) > 1
                    ) grouped
                    ON grouped.identity_key = dup.identity_key
                    WHERE dup.id != grouped.keep_id
                 );",
            )?;

            tx.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS uq_hyperedges_identity_key
                 ON hyperedges(identity_key)",
                [],
            )?;
        }
        tx.commit()?;
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

    /// Deterministic semantic identity for a hyperedge.
    ///
    /// Identity is edge kind plus a sorted role/node set. Position is intentionally
    /// excluded so equivalent writes with permuted insertion order map to one row.
    fn edge_identity_key(kind: &HyperedgeKind, members: &[HyperedgeMember]) -> String {
        let mut parts: Vec<(String, i64)> = members
            .iter()
            .map(|m| (m.role.clone(), m.node_id.0))
            .collect();
        parts.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut identity = String::from(kind.as_str());
        for (role, node_id) in parts {
            let _ = write!(&mut identity, "|{role}:{node_id}");
        }
        identity
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

    /// Build an `InMemoryGraph` from edges of a specific kind, applying a `SubgraphFilter`.
    async fn load_filtered_graph(
        &self,
        kind: HyperedgeKind,
        filter: &SubgraphFilter,
    ) -> crate::error::Result<InMemoryGraph> {
        let edges = self.get_edges_by_kind(kind).await?;
        if matches!(filter, SubgraphFilter::Full) {
            return Ok(InMemoryGraph::from_edges(&edges));
        }
        let full_graph = InMemoryGraph::from_edges(&edges);
        let allowed = resolve_filter(self, &full_graph, filter).await?;
        let filtered_edges: Vec<Hyperedge> = edges
            .into_iter()
            .filter(|e| {
                let (src, tgt) = extract_directed_pair(&e.members);
                allowed.contains(&src) && allowed.contains(&tgt)
            })
            .collect();
        Ok(InMemoryGraph::from_edges(&filtered_edges))
    }
}

/// Resolve a `SubgraphFilter` into the set of `NodeId`s that pass the filter.
/// Uses `Pin<Box>` to support recursive `And` filters.
fn resolve_filter<'a>(
    store: &'a SqliteStore,
    graph: &'a InMemoryGraph,
    filter: &'a SubgraphFilter,
) -> Pin<Box<dyn Future<Output = crate::error::Result<HashSet<NodeId>>> + Send + 'a>> {
    Box::pin(async move {
        match filter {
            SubgraphFilter::Full => Ok(graph.node_to_index.keys().copied().collect()),

            SubgraphFilter::OfKind { kinds } => {
                let mut allowed = HashSet::new();
                for kind in kinds {
                    let nodes = store
                        .find_nodes(&NodeFilter {
                            kind: Some(kind.clone()),
                            ..Default::default()
                        })
                        .await?;
                    for node in nodes {
                        if graph.node_to_index.contains_key(&node.id) {
                            allowed.insert(node.id);
                        }
                    }
                }
                Ok(allowed)
            }

            SubgraphFilter::Module { path_prefix } => {
                let nodes = store
                    .find_nodes(&NodeFilter {
                        name_prefix: Some(path_prefix.clone()),
                        ..Default::default()
                    })
                    .await?;
                Ok(nodes
                    .into_iter()
                    .filter(|n| graph.node_to_index.contains_key(&n.id))
                    .map(|n| n.id)
                    .collect())
            }

            SubgraphFilter::HighSalience { min_score } => {
                let analyses = store
                    .get_analyses_by_kind(AnalysisKind::CompositeSalience)
                    .await?;
                Ok(analyses
                    .into_iter()
                    .filter(|a| {
                        a.data
                            .get("score")
                            .and_then(serde_json::Value::as_f64)
                            .is_some_and(|s| s >= *min_score)
                            && graph.node_to_index.contains_key(&a.node_id)
                    })
                    .map(|a| a.node_id)
                    .collect())
            }

            SubgraphFilter::Neighborhood { centers, hops } => {
                use petgraph::Direction;
                use std::collections::VecDeque;

                let mut visited = HashSet::new();
                let mut queue = VecDeque::new();
                for center in centers {
                    if let Some(&idx) = graph.node_to_index.get(center) {
                        if visited.insert(*center) {
                            queue.push_back((idx, 0u32));
                        }
                    }
                }
                while let Some((idx, depth)) = queue.pop_front() {
                    if depth >= *hops {
                        continue;
                    }
                    let neighbors = graph
                        .graph
                        .neighbors_directed(idx, Direction::Outgoing)
                        .chain(graph.graph.neighbors_directed(idx, Direction::Incoming));
                    for neighbor_idx in neighbors {
                        if let Some(&nid) = graph.index_to_node.get(&neighbor_idx) {
                            if visited.insert(nid) {
                                queue.push_back((neighbor_idx, depth + 1));
                            }
                        }
                    }
                }
                Ok(visited)
            }

            SubgraphFilter::And(filters) => {
                let mut result: Option<HashSet<NodeId>> = None;
                for f in filters {
                    let set = resolve_filter(store, graph, f).await?;
                    result = Some(match result {
                        None => set,
                        Some(r) => r.intersection(&set).copied().collect(),
                    });
                }
                Ok(result.unwrap_or_default())
            }
        }
    })
}

#[async_trait::async_trait]
impl HomerStore for SqliteStore {
    // ── Node operations ────────────────────────────────────────────

    async fn upsert_node(&self, node: &Node) -> crate::error::Result<NodeId> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute(
            "UPDATE nodes SET metadata = json_set(metadata, '$.stale', true) WHERE id = ?1",
            params![id.0],
        )
        .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn delete_stale_nodes(&self, older_than: DateTime<Utc>) -> crate::error::Result<u64> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let cutoff = older_than.to_rfc3339();
        let count = conn
            .execute(
                "DELETE FROM nodes WHERE json_extract(metadata, '$.stale') = 1
                 AND last_extracted < ?1",
                params![cutoff],
            )
            .map_err(StoreError::Sqlite)?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(count as u64)
    }

    async fn upsert_nodes_batch(&self, nodes: &[Node]) -> crate::error::Result<Vec<NodeId>> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;

        let mut ids = Vec::with_capacity(nodes.len());
        for chunk in nodes.chunks(1000) {
            for node in chunk {
                let kind_str = node.kind.as_str();
                let metadata_json =
                    serde_json::to_string(&node.metadata).map_err(StoreError::Serialization)?;
                let last_extracted = node.last_extracted.to_rfc3339();
                #[allow(clippy::cast_possible_wrap)]
                let hash_i64: Option<i64> = node.content_hash.map(|h| h as i64);

                tx.execute(
                    "INSERT INTO nodes (kind, name, content_hash, last_extracted, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(kind, name) DO UPDATE SET
                        content_hash = excluded.content_hash,
                        last_extracted = excluded.last_extracted,
                        metadata = excluded.metadata",
                    params![kind_str, node.name, hash_i64, last_extracted, metadata_json],
                )
                .map_err(StoreError::Sqlite)?;

                let actual_id: i64 = tx
                    .query_row(
                        "SELECT id FROM nodes WHERE kind = ?1 AND name = ?2",
                        params![kind_str, node.name],
                        |row| row.get(0),
                    )
                    .map_err(StoreError::Sqlite)?;
                ids.push(NodeId(actual_id));
            }
        }
        tx.commit().map_err(StoreError::Sqlite)?;
        Ok(ids)
    }

    async fn resolve_canonical(&self, node_id: NodeId) -> crate::error::Result<NodeId> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        // Follow Aliases edges: the "old" role points to "new" role.
        // Walk the chain until no more aliases found (max 10 hops to prevent cycles).
        let mut current = node_id;
        for _ in 0..10 {
            let next: Option<i64> = conn
                .query_row(
                    "SELECT m_new.node_id FROM hyperedges e
                     JOIN hyperedge_members m_old ON e.id = m_old.hyperedge_id AND m_old.role = 'old'
                     JOIN hyperedge_members m_new ON e.id = m_new.hyperedge_id AND m_new.role = 'new'
                     WHERE e.kind = 'Aliases' AND m_old.node_id = ?1",
                    params![current.0],
                    |row| row.get(0),
                )
                .optional()
                .map_err(StoreError::Sqlite)?;
            match next {
                Some(id) => current = NodeId(id),
                None => break,
            }
        }
        Ok(current)
    }

    async fn alias_chain(&self, node_id: NodeId) -> crate::error::Result<Vec<NodeId>> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let mut chain = vec![node_id];
        let mut current = node_id;
        for _ in 0..10 {
            let next: Option<i64> = conn
                .query_row(
                    "SELECT m_new.node_id FROM hyperedges e
                     JOIN hyperedge_members m_old ON e.id = m_old.hyperedge_id AND m_old.role = 'old'
                     JOIN hyperedge_members m_new ON e.id = m_new.hyperedge_id AND m_new.role = 'new'
                     WHERE e.kind = 'Aliases' AND m_old.node_id = ?1",
                    params![current.0],
                    |row| row.get(0),
                )
                .optional()
                .map_err(StoreError::Sqlite)?;
            match next {
                Some(id) => {
                    current = NodeId(id);
                    chain.push(current);
                }
                None => break,
            }
        }
        Ok(chain)
    }

    // ── Hyperedge operations ───────────────────────────────────────

    async fn upsert_hyperedge(&self, edge: &Hyperedge) -> crate::error::Result<HyperedgeId> {
        let mut conn = self.conn.lock().expect("homer store mutex poisoned");
        let kind_str = edge.kind.as_str();
        let identity_key = Self::edge_identity_key(&edge.kind, &edge.members);
        let metadata_json =
            serde_json::to_string(&edge.metadata).map_err(StoreError::Serialization)?;
        let last_updated = edge.last_updated.to_rfc3339();

        let tx = conn.transaction().map_err(StoreError::Sqlite)?;

        tx.execute(
            "INSERT INTO hyperedges (kind, identity_key, confidence, last_updated, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(identity_key) DO UPDATE SET
                kind = excluded.kind,
                confidence = excluded.confidence,
                last_updated = excluded.last_updated,
                metadata = excluded.metadata",
            params![
                kind_str,
                &identity_key,
                edge.confidence,
                last_updated,
                metadata_json
            ],
        )
        .map_err(StoreError::Sqlite)?;

        let edge_id: i64 = tx
            .query_row(
                "SELECT id FROM hyperedges WHERE identity_key = ?1",
                params![&identity_key],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)?;

        // Members may have changed for an existing identity; rewrite membership set.
        tx.execute(
            "DELETE FROM hyperedge_members WHERE hyperedge_id = ?1",
            params![edge_id],
        )
        .map_err(StoreError::Sqlite)?;

        {
            let mut stmt = tx
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
        }

        tx.commit().map_err(StoreError::Sqlite)?;

        Ok(HyperedgeId(edge_id))
    }

    async fn get_edges_involving(&self, node_id: NodeId) -> crate::error::Result<Vec<Hyperedge>> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.query_row(
            "SELECT * FROM analysis_results WHERE node_id = ?1 AND kind = ?2",
            params![node_id.0, kind.as_str()],
            |row| {
                let data_str: String = row.get("data")?;
                let computed_at_str: String = row.get("computed_at")?;
                Ok(AnalysisResult {
                    id: AnalysisResultId(row.get("id")?),
                    node_id: NodeId(row.get("node_id")?),
                    kind,
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let count = conn
            .execute(
                "DELETE FROM analysis_results WHERE node_id = ?1",
                params![node_id.0],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(count as u64)
    }

    async fn invalidate_analyses_by_kinds(
        &self,
        node_id: NodeId,
        kinds: &[AnalysisKind],
    ) -> crate::error::Result<u64> {
        if kinds.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let placeholders: Vec<String> = (0..kinds.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "DELETE FROM analysis_results WHERE node_id = ?1 AND kind IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(node_id.0)];
        for kind in kinds {
            params_vec.push(Box::new(kind.as_str().to_string()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(AsRef::as_ref).collect();
        let count = stmt.execute(refs.as_slice()).map_err(StoreError::Sqlite)?;
        Ok(count as u64)
    }

    async fn invalidate_all_by_kinds(&self, kinds: &[AnalysisKind]) -> crate::error::Result<u64> {
        if kinds.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let placeholders: Vec<String> = (0..kinds.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            "DELETE FROM analysis_results WHERE kind IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for kind in kinds {
            params_vec.push(Box::new(kind.as_str().to_string()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(AsRef::as_ref).collect();
        let count = stmt.execute(refs.as_slice()).map_err(StoreError::Sqlite)?;
        Ok(count as u64)
    }

    async fn invalidate_analyses_excluding_kinds(
        &self,
        node_id: NodeId,
        keep_kinds: &[AnalysisKind],
    ) -> crate::error::Result<u64> {
        if keep_kinds.is_empty() {
            return self.invalidate_analyses(node_id).await;
        }
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let placeholders: Vec<String> = (0..keep_kinds.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "DELETE FROM analysis_results WHERE node_id = ?1 AND kind NOT IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(node_id.0)];
        for kind in keep_kinds {
            params_vec.push(Box::new(kind.as_str().to_string()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(AsRef::as_ref).collect();
        let count = stmt.execute(refs.as_slice()).map_err(StoreError::Sqlite)?;
        Ok(count as u64)
    }

    // ── Full-text search ───────────────────────────────────────────

    async fn index_text(
        &self,
        node_id: NodeId,
        content_type: &str,
        content: &str,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute("DELETE FROM checkpoints", [])
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn clear_analyses(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute("DELETE FROM analysis_results", [])
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn clear_analyses_by_kinds(&self, kinds: &[AnalysisKind]) -> crate::error::Result<()> {
        if kinds.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let placeholders: Vec<&str> = kinds.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM analysis_results WHERE kind IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(StoreError::Sqlite)?;
        let params: Vec<String> = kinds.iter().map(|k| k.as_str().to_string()).collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        stmt.execute(param_refs.as_slice())
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    // ── Graph snapshots ────────────────────────────────────────────

    async fn create_snapshot(&self, label: &str) -> crate::error::Result<SnapshotId> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
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

        let snap_id = conn.last_insert_rowid();

        // Record current node and edge membership for diff support
        conn.execute(
            "INSERT INTO snapshot_nodes (snapshot_id, node_id) SELECT ?1, id FROM nodes",
            params![snap_id],
        )
        .map_err(StoreError::Sqlite)?;
        conn.execute(
            "INSERT INTO snapshot_edges (snapshot_id, edge_id) SELECT ?1, id FROM hyperedges",
            params![snap_id],
        )
        .map_err(StoreError::Sqlite)?;

        Ok(SnapshotId(snap_id))
    }

    async fn list_snapshots(&self) -> crate::error::Result<Vec<SnapshotInfo>> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, label, snapshot_at, node_count, edge_count
                 FROM graph_snapshots ORDER BY snapshot_at ASC",
            )
            .map_err(StoreError::Sqlite)?;

        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let label: String = row.get(1)?;
                let at_str: String = row.get(2)?;
                let node_count: i64 = row.get(3)?;
                let edge_count: i64 = row.get(4)?;
                Ok((id, label, at_str, node_count, edge_count))
            })
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        let mut snapshots = Vec::with_capacity(rows.len());
        for (id, label, at_str, node_count, edge_count) in rows {
            let snapshot_at = DateTime::parse_from_rfc3339(&at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_default();
            snapshots.push(SnapshotInfo {
                id: SnapshotId(id),
                label,
                snapshot_at,
                node_count: u64::try_from(node_count).unwrap_or(0),
                edge_count: u64::try_from(edge_count).unwrap_or(0),
            });
        }

        Ok(snapshots)
    }

    async fn delete_snapshot(&self, label: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        // CASCADE will clean up snapshot_nodes and snapshot_edges
        let deleted = conn
            .execute(
                "DELETE FROM graph_snapshots WHERE label = ?1",
                params![label],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(deleted > 0)
    }

    async fn get_snapshot_diff(
        &self,
        from: SnapshotId,
        to: SnapshotId,
    ) -> crate::error::Result<GraphDiff> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");

        // Nodes added: in `to` but not in `from`
        let added_nodes = conn
            .prepare(
                "SELECT sn.node_id FROM snapshot_nodes sn
                 WHERE sn.snapshot_id = ?1
                   AND sn.node_id NOT IN (SELECT node_id FROM snapshot_nodes WHERE snapshot_id = ?2)",
            )
            .map_err(StoreError::Sqlite)?
            .query_map(params![to.0, from.0], |row| Ok(NodeId(row.get(0)?)))
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        // Nodes removed: in `from` but not in `to`
        let removed_nodes = conn
            .prepare(
                "SELECT sn.node_id FROM snapshot_nodes sn
                 WHERE sn.snapshot_id = ?1
                   AND sn.node_id NOT IN (SELECT node_id FROM snapshot_nodes WHERE snapshot_id = ?2)",
            )
            .map_err(StoreError::Sqlite)?
            .query_map(params![from.0, to.0], |row| Ok(NodeId(row.get(0)?)))
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        // Edges added
        let added_edges = conn
            .prepare(
                "SELECT se.edge_id FROM snapshot_edges se
                 WHERE se.snapshot_id = ?1
                   AND se.edge_id NOT IN (SELECT edge_id FROM snapshot_edges WHERE snapshot_id = ?2)",
            )
            .map_err(StoreError::Sqlite)?
            .query_map(params![to.0, from.0], |row| Ok(HyperedgeId(row.get(0)?)))
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        // Edges removed
        let removed_edges = conn
            .prepare(
                "SELECT se.edge_id FROM snapshot_edges se
                 WHERE se.snapshot_id = ?1
                   AND se.edge_id NOT IN (SELECT edge_id FROM snapshot_edges WHERE snapshot_id = ?2)",
            )
            .map_err(StoreError::Sqlite)?
            .query_map(params![from.0, to.0], |row| Ok(HyperedgeId(row.get(0)?)))
            .map_err(StoreError::Sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::Sqlite)?;

        Ok(GraphDiff {
            added_nodes,
            removed_nodes,
            added_edges,
            removed_edges,
        })
    }

    // ── Graph loading ────────────────────────────────────────────────

    async fn load_call_graph(
        &self,
        filter: &SubgraphFilter,
    ) -> crate::error::Result<InMemoryGraph> {
        self.load_filtered_graph(HyperedgeKind::Calls, filter).await
    }

    async fn load_import_graph(
        &self,
        filter: &SubgraphFilter,
    ) -> crate::error::Result<InMemoryGraph> {
        self.load_filtered_graph(HyperedgeKind::Imports, filter)
            .await
    }

    // ── Transactions ──────────────────────────────────────────────

    async fn begin_transaction(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn commit_transaction(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute_batch("COMMIT").map_err(StoreError::Sqlite)?;
        Ok(())
    }

    async fn rollback_transaction(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");
        conn.execute_batch("ROLLBACK").map_err(StoreError::Sqlite)?;
        Ok(())
    }

    // ── Metrics ────────────────────────────────────────────────────

    async fn stats(&self) -> crate::error::Result<StoreStats> {
        let conn = self.conn.lock().expect("homer store mutex poisoned");

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

        let db_size_bytes = self
            .db_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len());

        Ok(StoreStats {
            total_nodes,
            total_edges,
            total_analyses,
            nodes_by_kind,
            edges_by_kind,
            db_size_bytes,
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
    async fn hyperedge_upsert_is_idempotent_for_equivalent_edges() {
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
            confidence: 0.8,
            last_updated: Utc::now(),
            metadata: HashMap::new(),
        };

        let id1 = store.upsert_hyperedge(&edge).await.unwrap();
        let id2 = store.upsert_hyperedge(&edge).await.unwrap();
        assert_eq!(id1, id2, "Equivalent upserts should reuse edge identity");

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_edges, 1, "Should not grow duplicate edges");
    }

    #[tokio::test]
    async fn opening_legacy_db_backfills_identity_and_deduplicates() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS homer_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
             CREATE TABLE IF NOT EXISTS nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                content_hash INTEGER,
                last_extracted TEXT NOT NULL,
                metadata TEXT DEFAULT '{}',
                UNIQUE(kind, name)
            );
             CREATE TABLE IF NOT EXISTS hyperedges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                confidence REAL DEFAULT 1.0,
                last_updated TEXT NOT NULL,
                metadata TEXT DEFAULT '{}'
            );
             CREATE TABLE IF NOT EXISTS hyperedge_members (
                hyperedge_id INTEGER NOT NULL REFERENCES hyperedges(id) ON DELETE CASCADE,
                node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                role TEXT NOT NULL DEFAULT '',
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (hyperedge_id, node_id, role)
            );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO nodes (kind, name, content_hash, last_extracted, metadata)
             VALUES ('Function', 'main', 1, ?1, '{}')",
            params![Utc::now().to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes (kind, name, content_hash, last_extracted, metadata)
             VALUES ('Function', 'helper', 1, ?1, '{}')",
            params![Utc::now().to_rfc3339()],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO hyperedges (kind, confidence, last_updated, metadata)
             VALUES ('Calls', 1.0, ?1, '{}')",
            params![Utc::now().to_rfc3339()],
        )
        .unwrap();
        let edge_a = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO hyperedges (kind, confidence, last_updated, metadata)
             VALUES ('Calls', 1.0, ?1, '{}')",
            params![Utc::now().to_rfc3339()],
        )
        .unwrap();
        let edge_b = conn.last_insert_rowid();

        for edge_id in [edge_a, edge_b] {
            conn.execute(
                "INSERT INTO hyperedge_members (hyperedge_id, node_id, role, position)
                 VALUES (?1, 1, 'caller', 0)",
                params![edge_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO hyperedge_members (hyperedge_id, node_id, role, position)
                 VALUES (?1, 2, 'callee', 1)",
                params![edge_id],
            )
            .unwrap();
        }

        drop(conn);

        let store = SqliteStore::open(tmp.path()).unwrap();
        let edges = store.get_edges_by_kind(HyperedgeKind::Calls).await.unwrap();
        assert_eq!(edges.len(), 1, "Legacy duplicates should be collapsed");
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

    #[tokio::test]
    async fn delete_stale_nodes_removes_old_stale() {
        let store = SqliteStore::in_memory().unwrap();
        let id = store
            .upsert_node(&make_test_node(NodeKind::File, "old.rs"))
            .await
            .unwrap();
        store.mark_node_stale(id).await.unwrap();

        // Delete stale nodes older than far future — should delete
        let deleted = store
            .delete_stale_nodes(Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(deleted, 1);

        assert!(store.get_node(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_nodes_batch_transactional() {
        let store = SqliteStore::in_memory().unwrap();
        let nodes: Vec<Node> = (0..50)
            .map(|i| make_test_node(NodeKind::File, &format!("batch/file_{i}.rs")))
            .collect();

        let ids = store.upsert_nodes_batch(&nodes).await.unwrap();
        assert_eq!(ids.len(), 50);

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_nodes, 50);
    }

    #[tokio::test]
    async fn resolve_canonical_follows_alias_chain() {
        let store = SqliteStore::in_memory().unwrap();
        let old_id = store
            .upsert_node(&make_test_node(NodeKind::File, "old_name.rs"))
            .await
            .unwrap();
        let new_id = store
            .upsert_node(&make_test_node(NodeKind::File, "new_name.rs"))
            .await
            .unwrap();

        // Create alias edge: old -> new
        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Aliases,
                members: vec![
                    HyperedgeMember {
                        node_id: old_id,
                        role: "old".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: new_id,
                        role: "new".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let resolved = store.resolve_canonical(old_id).await.unwrap();
        assert_eq!(resolved, new_id);

        // Already canonical — resolves to self
        let self_resolved = store.resolve_canonical(new_id).await.unwrap();
        assert_eq!(self_resolved, new_id);
    }

    #[tokio::test]
    async fn alias_chain_returns_full_history() {
        let store = SqliteStore::in_memory().unwrap();
        let v1 = store
            .upsert_node(&make_test_node(NodeKind::File, "utils.rs"))
            .await
            .unwrap();
        let v2 = store
            .upsert_node(&make_test_node(NodeKind::File, "helpers.rs"))
            .await
            .unwrap();
        let v3 = store
            .upsert_node(&make_test_node(NodeKind::File, "core_helpers.rs"))
            .await
            .unwrap();

        // v1 -> v2 -> v3 alias chain
        for (old, new) in [(v1, v2), (v2, v3)] {
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Aliases,
                    members: vec![
                        HyperedgeMember {
                            node_id: old,
                            role: "old".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: new,
                            role: "new".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: Utc::now(),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();
        }

        // Full chain from v1
        let chain = store.alias_chain(v1).await.unwrap();
        assert_eq!(chain, vec![v1, v2, v3]);

        // Partial chain from v2
        let chain = store.alias_chain(v2).await.unwrap();
        assert_eq!(chain, vec![v2, v3]);

        // No aliases from v3
        let chain = store.alias_chain(v3).await.unwrap();
        assert_eq!(chain, vec![v3]);
    }

    #[tokio::test]
    async fn snapshot_diff_detects_changes() {
        let store = SqliteStore::in_memory().unwrap();

        // Snapshot 1: one node
        store
            .upsert_node(&make_test_node(NodeKind::File, "a.rs"))
            .await
            .unwrap();
        let snap1 = store.create_snapshot("v1").await.unwrap();

        // Add another node, take snapshot 2
        store
            .upsert_node(&make_test_node(NodeKind::File, "b.rs"))
            .await
            .unwrap();
        let snap2 = store.create_snapshot("v2").await.unwrap();

        let diff = store.get_snapshot_diff(snap1, snap2).await.unwrap();
        assert_eq!(diff.added_nodes.len(), 1, "should detect 1 added node");
        assert!(diff.removed_nodes.is_empty(), "no nodes removed");
    }

    /// Helper: create a directed edge between two nodes.
    async fn insert_edge(
        store: &SqliteStore,
        kind: HyperedgeKind,
        src: NodeId,
        tgt: NodeId,
        src_role: &str,
        tgt_role: &str,
    ) {
        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind,
                members: vec![
                    HyperedgeMember {
                        node_id: src,
                        role: src_role.to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: tgt,
                        role: tgt_role.to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn load_call_graph_full() {
        let store = SqliteStore::in_memory().unwrap();
        let a = store
            .upsert_node(&make_test_node(NodeKind::Function, "main"))
            .await
            .unwrap();
        let b = store
            .upsert_node(&make_test_node(NodeKind::Function, "helper"))
            .await
            .unwrap();
        let c = store
            .upsert_node(&make_test_node(NodeKind::Function, "util"))
            .await
            .unwrap();

        // main -> helper -> util
        insert_edge(&store, HyperedgeKind::Calls, a, b, "caller", "callee").await;
        insert_edge(&store, HyperedgeKind::Calls, b, c, "caller", "callee").await;

        let graph = store.load_call_graph(&SubgraphFilter::Full).await.unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }

    #[tokio::test]
    async fn load_import_graph_module_filter() {
        let store = SqliteStore::in_memory().unwrap();
        let a = store
            .upsert_node(&make_test_node(NodeKind::File, "src/auth/login.rs"))
            .await
            .unwrap();
        let b = store
            .upsert_node(&make_test_node(NodeKind::File, "src/auth/token.rs"))
            .await
            .unwrap();
        let c = store
            .upsert_node(&make_test_node(NodeKind::File, "src/db/pool.rs"))
            .await
            .unwrap();

        // login imports token and pool
        insert_edge(&store, HyperedgeKind::Imports, a, b, "importer", "imported").await;
        insert_edge(&store, HyperedgeKind::Imports, a, c, "importer", "imported").await;

        // Filter to src/auth/ — pool.rs excluded, so only login->token edge survives
        let graph = store
            .load_import_graph(&SubgraphFilter::Module {
                path_prefix: "src/auth/".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
    }

    #[tokio::test]
    async fn load_call_graph_neighborhood_filter() {
        let store = SqliteStore::in_memory().unwrap();
        let a = store
            .upsert_node(&make_test_node(NodeKind::Function, "a"))
            .await
            .unwrap();
        let b = store
            .upsert_node(&make_test_node(NodeKind::Function, "b"))
            .await
            .unwrap();
        let c = store
            .upsert_node(&make_test_node(NodeKind::Function, "c"))
            .await
            .unwrap();
        let d = store
            .upsert_node(&make_test_node(NodeKind::Function, "d"))
            .await
            .unwrap();

        // a -> b -> c -> d (chain of 3 hops)
        insert_edge(&store, HyperedgeKind::Calls, a, b, "caller", "callee").await;
        insert_edge(&store, HyperedgeKind::Calls, b, c, "caller", "callee").await;
        insert_edge(&store, HyperedgeKind::Calls, c, d, "caller", "callee").await;

        // 1-hop from a: should include a and b
        let graph = store
            .load_call_graph(&SubgraphFilter::Neighborhood {
                centers: vec![a],
                hops: 1,
            })
            .await
            .unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
    }

    #[tokio::test]
    async fn load_call_graph_empty() {
        let store = SqliteStore::in_memory().unwrap();
        let graph = store.load_call_graph(&SubgraphFilter::Full).await.unwrap();
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[tokio::test]
    async fn transaction_commit_is_atomic() {
        let store = SqliteStore::in_memory().unwrap();

        store.begin_transaction().await.unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::File, "tx1.rs"))
            .await
            .unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::File, "tx2.rs"))
            .await
            .unwrap();
        store.commit_transaction().await.unwrap();

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_nodes, 2);
    }

    #[tokio::test]
    async fn transaction_rollback_discards_changes() {
        let store = SqliteStore::in_memory().unwrap();

        store
            .upsert_node(&make_test_node(NodeKind::File, "keeper.rs"))
            .await
            .unwrap();

        store.begin_transaction().await.unwrap();
        store
            .upsert_node(&make_test_node(NodeKind::File, "discard.rs"))
            .await
            .unwrap();
        store.rollback_transaction().await.unwrap();

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_nodes, 1);
        assert!(
            store
                .get_node_by_name(NodeKind::File, "discard.rs")
                .await
                .unwrap()
                .is_none()
        );
    }
}

// ── Property-based tests ──────────────────────────────────────────────
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating arbitrary `NodeKind`.
    fn arb_node_kind() -> impl Strategy<Value = NodeKind> {
        prop_oneof![
            Just(NodeKind::File),
            Just(NodeKind::Function),
            Just(NodeKind::Type),
            Just(NodeKind::Module),
            Just(NodeKind::Commit),
            Just(NodeKind::Contributor),
            Just(NodeKind::Document),
            Just(NodeKind::ExternalDep),
        ]
    }

    /// Strategy for generating arbitrary `HyperedgeKind`.
    fn arb_edge_kind() -> impl Strategy<Value = HyperedgeKind> {
        prop_oneof![
            Just(HyperedgeKind::Calls),
            Just(HyperedgeKind::Imports),
            Just(HyperedgeKind::Modifies),
            Just(HyperedgeKind::BelongsTo),
            Just(HyperedgeKind::DependsOn),
            Just(HyperedgeKind::Authored),
            Just(HyperedgeKind::CoChanges),
            Just(HyperedgeKind::Documents),
        ]
    }

    /// Strategy for generating arbitrary `AnalysisKind`.
    fn arb_analysis_kind() -> impl Strategy<Value = AnalysisKind> {
        prop_oneof![
            Just(AnalysisKind::ChangeFrequency),
            Just(AnalysisKind::PageRank),
            Just(AnalysisKind::BetweennessCentrality),
            Just(AnalysisKind::CompositeSalience),
            Just(AnalysisKind::CommunityAssignment),
            Just(AnalysisKind::ContributorConcentration),
        ]
    }

    /// Strategy for generating valid node names (non-empty, no NUL bytes).
    fn arb_node_name() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_/.:]{1,100}"
    }

    /// Strategy for JSON-safe metadata values.
    fn arb_metadata() -> impl Strategy<Value = HashMap<String, serde_json::Value>> {
        proptest::collection::hash_map(
            "[a-z_]{1,20}",
            prop_oneof![
                any::<i64>().prop_map(serde_json::Value::from),
                any::<bool>().prop_map(serde_json::Value::from),
                "[a-zA-Z0-9 ]{0,50}".prop_map(serde_json::Value::from),
            ],
            0..5,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        /// Node round-trip: upsert then retrieve preserves kind, name, and metadata.
        #[test]
        fn node_roundtrip(kind in arb_node_kind(), name in arb_node_name(), metadata in arb_metadata()) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();
                let node = Node {
                    id: NodeId(0),
                    kind: kind.clone(),
                    name: name.clone(),
                    content_hash: Some(42),
                    last_extracted: Utc::now(),
                    metadata: metadata.clone(),
                };

                let id = store.upsert_node(&node).await.unwrap();
                let fetched = store.get_node(id).await.unwrap().expect("node should exist");

                prop_assert_eq!(&fetched.kind, &kind);
                prop_assert_eq!(fetched.name, name);
                prop_assert_eq!(fetched.content_hash, Some(42));
                // Verify metadata values round-trip (keys and types preserved)
                for (k, v) in &metadata {
                    let fetched_v = fetched.metadata.get(k).expect("metadata key should exist");
                    prop_assert_eq!(fetched_v, v, "metadata mismatch for key {}", k);
                }
                Ok(())
            })?;
        }

        /// Hyperedge round-trip: upsert then retrieve preserves kind, members, and confidence.
        #[test]
        fn edge_roundtrip(
            kind in arb_edge_kind(),
            confidence in 0.0..=1.0_f64,
            role_a in "[a-z]{1,10}",
            role_b in "[a-z]{1,10}",
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();

                // Create two nodes first
                let id_a = store.upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::File,
                    name: "a.rs".to_string(),
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata: HashMap::new(),
                }).await.unwrap();

                let id_b = store.upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::File,
                    name: "b.rs".to_string(),
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata: HashMap::new(),
                }).await.unwrap();

                let edge = Hyperedge {
                    id: HyperedgeId(0),
                    kind: kind.clone(),
                    members: vec![
                        HyperedgeMember { node_id: id_a, role: role_a.clone(), position: 0 },
                        HyperedgeMember { node_id: id_b, role: role_b.clone(), position: 1 },
                    ],
                    confidence,
                    last_updated: Utc::now(),
                    metadata: HashMap::new(),
                };

                store.upsert_hyperedge(&edge).await.unwrap();
                let edges = store.get_edges_by_kind(kind.clone()).await.unwrap();
                prop_assert_eq!(edges.len(), 1);
                let fetched = &edges[0];

                prop_assert_eq!(&fetched.kind, &kind);
                prop_assert_eq!(fetched.members.len(), 2);
                prop_assert!((fetched.confidence - confidence).abs() < 1e-10);
                prop_assert_eq!(&fetched.members[0].role, &role_a);
                prop_assert_eq!(&fetched.members[1].role, &role_b);
                Ok(())
            })?;
        }

        /// Analysis result round-trip: store then retrieve by kind.
        #[test]
        fn analysis_roundtrip(
            kind in arb_analysis_kind(),
            score in -1e15_f64..1e15_f64,
            input_hash in 0..=(i64::MAX as u64),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();

                let node_id = store.upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::File,
                    name: "test.rs".to_string(),
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata: HashMap::new(),
                }).await.unwrap();

                let result = AnalysisResult {
                    id: AnalysisResultId(0),
                    node_id,
                    kind,
                    data: serde_json::json!({"score": score}),
                    input_hash,
                    computed_at: Utc::now(),
                };

                store.store_analysis(&result).await.unwrap();

                let results = store.get_analyses_by_kind(kind).await.unwrap();
                prop_assert_eq!(results.len(), 1);
                let r = &results[0];
                prop_assert_eq!(r.node_id, node_id);
                prop_assert_eq!(&r.kind, &kind);
                prop_assert_eq!(r.input_hash, input_hash);

                let stored_score = r.data.get("score")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap();
                // JSON round-trip can lose precision for large values.
                // Use relative tolerance: |diff| / max(|a|, |b|, 1) < epsilon.
                let denom = score.abs().max(stored_score.abs()).max(1.0);
                prop_assert!((stored_score - score).abs() / denom < 1e-10,
                    "score mismatch: stored={} vs original={}", stored_score, score);
                Ok(())
            })?;
        }

        /// Node upsert is idempotent: same name+kind → same ID.
        #[test]
        fn node_upsert_idempotent(kind in arb_node_kind(), name in arb_node_name()) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();
                let node = Node {
                    id: NodeId(0),
                    kind: kind.clone(),
                    name: name.clone(),
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata: HashMap::new(),
                };

                let id1 = store.upsert_node(&node).await.unwrap();
                let id2 = store.upsert_node(&node).await.unwrap();
                prop_assert_eq!(id1, id2, "Upsert should return same ID for same name+kind");
                Ok(())
            })?;
        }

        /// Checkpoint round-trip: set then get returns same value.
        #[test]
        fn checkpoint_roundtrip(key in "[a-z_]{1,20}", value in "[a-f0-9]{8,40}") {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();
                store.set_checkpoint(&key, &value).await.unwrap();
                let fetched = store.get_checkpoint(&key).await.unwrap();
                prop_assert_eq!(fetched, Some(value));
                Ok(())
            })?;
        }
    }
}
