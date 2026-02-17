/// Current schema version.
pub const SCHEMA_VERSION: &str = "1";

/// Full SQL schema for Homer's `SQLite` database.
pub const SCHEMA_SQL: &str = r"
-- Schema version tracking
CREATE TABLE IF NOT EXISTS homer_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- All nodes in the hypergraph
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    content_hash INTEGER,
    last_extracted TEXT NOT NULL,
    metadata TEXT DEFAULT '{}',
    UNIQUE(kind, name)
);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);

-- Hyperedges (the relationships)
CREATE TABLE IF NOT EXISTS hyperedges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    confidence REAL DEFAULT 1.0,
    last_updated TEXT NOT NULL,
    metadata TEXT DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_hyperedges_kind ON hyperedges(kind);

-- Membership in hyperedges (junction table)
CREATE TABLE IF NOT EXISTS hyperedge_members (
    hyperedge_id INTEGER NOT NULL REFERENCES hyperedges(id) ON DELETE CASCADE,
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hyperedge_id, node_id, role)
);
CREATE INDEX IF NOT EXISTS idx_hem_node ON hyperedge_members(node_id);
CREATE INDEX IF NOT EXISTS idx_hem_edge ON hyperedge_members(hyperedge_id);

-- Analysis results (derived data)
CREATE TABLE IF NOT EXISTS analysis_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    data TEXT NOT NULL,
    input_hash INTEGER NOT NULL,
    computed_at TEXT NOT NULL,
    UNIQUE(node_id, kind)
);
CREATE INDEX IF NOT EXISTS idx_ar_node ON analysis_results(node_id);
CREATE INDEX IF NOT EXISTS idx_ar_kind ON analysis_results(kind);

-- Full-text search index over text content
CREATE VIRTUAL TABLE IF NOT EXISTS text_search USING fts5(
    node_id,
    content_type,
    content,
    tokenize='porter unicode61'
);

-- Incrementality checkpoints
CREATE TABLE IF NOT EXISTS checkpoints (
    kind TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Graph snapshots for temporal analysis
CREATE TABLE IF NOT EXISTS graph_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    snapshot_at TEXT NOT NULL,
    edge_count INTEGER NOT NULL,
    node_count INTEGER NOT NULL
);
";

/// Projected views for common query patterns.
pub const VIEWS_SQL: &str = r"
-- Call graph as simple (caller, callee) pairs
CREATE VIEW IF NOT EXISTS call_graph AS
SELECT
    caller.node_id AS caller_id,
    callee.node_id AS callee_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members caller ON e.id = caller.hyperedge_id AND caller.role = 'caller'
JOIN hyperedge_members callee ON e.id = callee.hyperedge_id AND callee.role = 'callee'
WHERE e.kind = 'Calls';

-- Import graph as simple (importer, imported) pairs
CREATE VIEW IF NOT EXISTS import_graph AS
SELECT
    src.node_id AS importer_id,
    tgt.node_id AS imported_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members src ON e.id = src.hyperedge_id AND src.role = 'importer'
JOIN hyperedge_members tgt ON e.id = tgt.hyperedge_id AND tgt.role = 'imported'
WHERE e.kind = 'Imports';

-- Files modified by each commit
CREATE VIEW IF NOT EXISTS commit_files AS
SELECT
    c.node_id AS commit_id,
    f.node_id AS file_id,
    c.role AS commit_role,
    f.role AS file_role
FROM hyperedges e
JOIN hyperedge_members c ON e.id = c.hyperedge_id AND c.role = 'commit'
JOIN hyperedge_members f ON e.id = f.hyperedge_id AND f.role = 'file'
WHERE e.kind = 'Modifies';

-- Documents and the code entities they reference
CREATE VIEW IF NOT EXISTS document_references AS
SELECT
    d.node_id AS document_id,
    c.node_id AS code_entity_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members d ON e.id = d.hyperedge_id AND d.role = 'document'
JOIN hyperedge_members c ON e.id = c.hyperedge_id AND c.role = 'code_entity'
WHERE e.kind = 'Documents';
";

/// `SQLite` PRAGMAs for performance.
pub const PRAGMAS_SQL: &str = r"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -64000;
PRAGMA mmap_size = 268435456;
PRAGMA foreign_keys = ON;
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_executes_on_in_memory_sqlite() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        // Execute pragmas (skip WAL for in-memory)
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // Execute schema
        conn.execute_batch(SCHEMA_SQL).unwrap();

        // Execute views
        conn.execute_batch(VIEWS_SQL).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"nodes".to_string()));
        assert!(tables.contains(&"hyperedges".to_string()));
        assert!(tables.contains(&"hyperedge_members".to_string()));
        assert!(tables.contains(&"analysis_results".to_string()));
        assert!(tables.contains(&"checkpoints".to_string()));
        assert!(tables.contains(&"graph_snapshots".to_string()));
        assert!(tables.contains(&"homer_meta".to_string()));
    }

    #[test]
    fn schema_version_is_set() {
        assert_eq!(SCHEMA_VERSION, "1");
    }
}
