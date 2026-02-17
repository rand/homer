# Homer Store: Hypergraph Persistence

> Data model, SQLite schema, incrementality protocol, and query patterns.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [EXTRACTORS.md](EXTRACTORS.md) · [ANALYZERS.md](ANALYZERS.md) · [PERFORMANCE.md](PERFORMANCE.md) · [EVOLUTION.md](EVOLUTION.md)  
**Inspired by**: Hypergraph memory in [loop](https://github.com/rand/loop) (SQLite-backed knowledge store with tiered lifecycle)

---

## Why a Hypergraph

A standard graph connects exactly two nodes per edge. Many of Homer's core relationships are **n-ary**:

| Relationship | Arity | Example |
|-------------|-------|---------|
| Commit modifies files | 1:N | Commit `abc123` modified `{auth.rs, middleware.rs, tests/auth_test.rs}` |
| Co-change set | N | Files `{A, B, C}` change together 87% of the time |
| PR resolves issues | 1:N | PR #42 resolved issues `{#13, #17, #22}` |
| Function calls functions | 1:N | `process_order()` calls `{validate(), charge(), ship()}` |
| Community membership | N | Functions `{f1, f2, f3, f4}` form a community cluster |
| Document references code | 1:N | README references `{auth.rs, validate_token(), UserModel}` |
| Prompt modifies files | 1:N | Agent session modified `{handler.rs, schema.rs, test_handler.rs}` |

Decomposing these into binary edges loses the "togetherness" semantics. A hyperedge preserves the joint relationship: knowing that `{A, B, C}` always change *together* is different from knowing that A-B, B-C, and A-C each co-change pairwise.

---

## Entity Glossary

Homer's node types map to concepts that vary across languages. This glossary defines what each type means in Homer's model and how language-specific constructs map to it.

| Homer Entity | Definition | Examples by Language |
|-------------|-----------|---------------------|
| **File** | A source file tracked by git. Identity = repository-relative path at a given snapshot. | `src/auth.rs`, `lib/auth.py`, `src/auth.ts` |
| **Function** | Any callable unit of code: named function, method, closure/lambda (only if named or assigned), macro that generates code. Excludes: anonymous inline closures, property getters/setters (unless explicitly defined as methods). | Rust: `fn`, `impl` method. Python: `def`, `async def`. TypeScript: `function`, arrow function assigned to `const`. Go: `func`. |
| **Type** | A named type declaration: struct, class, enum, interface, type alias, trait/protocol. | Rust: `struct`, `enum`, `trait`. Python: `class`. TypeScript: `interface`, `type`, `class`. Go: `type ... struct`. |
| **Module** | A namespace or organizational unit. Maps to the language's module/package system. In file-per-module languages (Rust, Python), a Module node may correspond to a file. Homer deduplicates: a file that *is* a module gets one File node and one Module node linked by `BelongsTo`. | Rust: `mod`. Python: module (file) or package (`__init__.py`). TypeScript: file or `namespace`. Go: `package`. |
| **Snapshot** | An immutable point-in-time view of the repository (typically HEAD, but configurable). All node properties and graph edges are relative to a snapshot. A new analysis run creates a new snapshot; nodes unchanged since the prior snapshot are carried forward via content hashing. |

**Key invariants**:
- Every Function and Type node belongs to exactly one Module (via `BelongsTo`)
- Every File node belongs to at most one Module (files outside detected modules get a synthetic root module)
- Node identity is (kind, qualified_name, snapshot). Across snapshots, the `Aliases` edge links renamed/moved entities
- Hyperedges are immutable once created. Analysis updates create new edges and mark old ones stale rather than mutating in place

---

## Data Model

### Node Types

```rust
/// Every entity Homer tracks is a node in the hypergraph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    // --- Code entities ---
    
    /// A source file
    File,
    /// A function, method, or callable
    Function,
    /// A type, class, struct, enum, or interface
    Type,
    /// A module or package (may map to directory or namespace)
    Module,
    
    // --- Git/forge entities ---
    
    /// A git commit
    Commit,
    /// A pull request
    PullRequest,
    /// An issue
    Issue,
    /// A contributor (author, reviewer)
    Contributor,
    /// A git tag or release
    Release,
    
    // --- Derived entities ---
    
    /// A recovered architectural concept (from community detection + LLM)
    Concept,
    /// An external dependency (from package manifests)
    ExternalDep,
    
    // --- Documentation entities ---
    
    /// A standalone documentation file (README, ADR, CONTRIBUTING, etc.)
    /// Note: doc comments are metadata on Function/Type/Module nodes, not separate nodes
    Document,
    
    // --- Agent interaction entities ---
    
    /// A developer prompt or instruction to an AI agent
    Prompt,
    /// A curated agent context file (CLAUDE.md, .cursor/rules, etc.)
    AgentRule,
    /// An agent session (group of related prompts)
    AgentSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Canonical name: file path, function qualified name, commit SHA, etc.
    pub name: String,
    /// Content hash for change detection (source hash for code, message hash for commits)
    pub content_hash: Option<u64>,
    /// When this node was last extracted/updated
    pub last_extracted: DateTime<Utc>,
    /// Arbitrary key-value metadata
    pub metadata: HashMap<String, Value>,
}
```

**NodeId**: Typed wrapper around `i64` (SQLite rowid). Different from content-based identity — a function that gets renamed creates a new node, and the old node is marked stale.

### Entity Identity and Aliasing

Renames, file moves, and symbol evolution create discontinuities in longitudinal analysis. Without aliasing, a file moved from `src/lib.rs` to `src/core/lib.rs` loses its entire change history in Homer's view.

**Aliasing model**: Homer tracks identity across renames via `Aliases` hyperedges (see HyperedgeKind below). When a git diff detects a rename (via `libgit2`'s similarity-based rename detection), both the old and new nodes are created, and an `Aliases` edge links them with confidence metadata.

```rust
/// Alias metadata stored on Aliases hyperedges
pub struct AliasMetadata {
    /// How the alias was detected
    pub detection: AliasDetection,
    /// Confidence in the alias relationship (0.0–1.0)
    pub confidence: f64,
    /// Commit where the rename was observed
    pub observed_at: CommitId,
}

pub enum AliasDetection {
    /// Git detected rename via diff similarity (threshold configurable, default 50%)
    GitRename { similarity: f64 },
    /// Symbol renamed but file unchanged (tree-sitter structural match)
    SymbolRename,
    /// Manual alias declared in configuration
    Configured,
}
```

**Alias collapse for trend analysis**: Analyzers that compute time-series metrics (change frequency trends, contributor evolution) must resolve alias chains before aggregation. The store provides a helper:

```rust
/// Resolve a node to its canonical identity by following Aliases edges.
/// Returns the newest (non-stale) node in the alias chain.
async fn resolve_canonical(&self, node_id: NodeId) -> Result<NodeId>;

/// Get the full alias chain for a node (all historical identities).
async fn alias_chain(&self, node_id: NodeId) -> Result<Vec<NodeId>>;
```

Alias resolution is **best-effort**: git rename detection has a similarity threshold (default 50%), and cross-file symbol renames may be missed if the extractor tier doesn't support full cross-module resolution. Analyzers should document whether they collapse aliases and what happens when alias chains break.

### Doc Comment Metadata

Doc comments are stored as **metadata on existing `Function`, `Type`, and `Module` nodes**, not as separate nodes. Creating a node and edge for what is fundamentally an attribute of a code entity would overcomplicate the graph and inflate node counts.

During tree-sitter parsing in the graph extractor, doc comments adjacent to definitions are captured and stored in the node's metadata:

```rust
/// Metadata fields stored on Function/Type/Module nodes for doc comments.
/// These are stored in the node's `metadata` HashMap.
pub struct CodeEntityDocMetadata {
    /// The doc comment text, stripped of syntax markers (///, /** */, #, etc.)
    pub doc_comment: Option<String>,
    /// Hash of the doc comment for freshness tracking
    pub doc_comment_hash: Option<u64>,
    /// Documentation style detected (rustdoc, jsdoc, numpy, etc.)
    pub doc_style: Option<DocStyle>,
}

pub enum DocStyle {
    Rustdoc,
    Jsdoc,
    Numpy,
    Google,
    Sphinx,
    Javadoc,
    Godoc,
    Other(String),
}
```

This design reserves the full `Node(Document)` + `Documents` hyperedge treatment for **standalone documentation files** (README, ADR, guides) that have their own identity and cross-reference relationships. Inline doc comments are *attributes of code*, not *entities in the knowledge graph*.

### Hyperedge Types

```rust
/// A hyperedge connects one or more nodes with a typed relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HyperedgeKind {
    // --- Extraction-derived edges ---
    
    /// A commit modified these files (source: git extractor)
    Modifies,
    /// A file imports from these files/modules (source: graph extractor)
    Imports,
    /// A function/method calls these functions (source: graph extractor)
    Calls,
    /// A type inherits from / implements these types (source: graph extractor)
    Inherits,
    /// A PR resolved these issues (source: GitHub extractor)
    Resolves,
    /// A contributor authored these commits (source: git extractor)
    Authored,
    /// A contributor reviewed these PRs (source: GitHub extractor)
    Reviewed,
    /// A release includes these commits (source: git extractor)
    Includes,
    /// A file belongs to this module (source: structure extractor)
    BelongsTo,
    /// A module depends on this external dependency (source: manifest parser)
    DependsOn,
    /// Two nodes represent the same entity across a rename/move (source: git extractor, graph extractor)
    /// Metadata includes detection method, confidence, and the commit where the rename was observed.
    Aliases,
    
    // --- Document-derived edges ---
    
    /// A document references these code entities (source: document extractor)
    /// Links Document nodes to Function/Type/Module/File nodes via parsed
    /// Markdown links, backtick-quoted identifiers, and file path mentions
    Documents,
    
    // --- Prompt-derived edges ---
    
    /// A prompt/session references these code entities (source: prompt extractor)
    PromptReferences,
    /// A prompt resulted in changes to these files (high-value signal)
    PromptModifiedFiles,
    /// These prompts address the same area/concern (source: prompt analyzer)
    RelatedPrompts,
    
    // --- Analysis-derived edges ---
    
    /// These files co-change above a threshold (source: behavioral analyzer)
    CoChanges,
    /// These nodes form a community cluster (source: community detection)
    ClusterMembers,
    /// This concept encompasses these implementation nodes (source: semantic analyzer)
    Encompasses,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hyperedge {
    pub id: HyperedgeId,
    pub kind: HyperedgeKind,
    /// The nodes participating in this relationship
    pub members: Vec<HyperedgeMember>,
    /// Edge-level confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// When this edge was created/last updated
    pub last_updated: DateTime<Utc>,
    /// Arbitrary key-value metadata (e.g., co-change frequency, call count)
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperedgeMember {
    pub node_id: NodeId,
    /// Role within this edge (e.g., "caller"/"callee", "author"/"commit")
    pub role: String,
    /// Ordering within the edge (meaningful for some edge types)
    pub position: u32,
}
```

**Note on document cross-references**: Standalone documents that reference the same code entities are implicitly connected *through* those shared code entity nodes (two-hop reachability in the hypergraph). No explicit `CoDocuments` edge is needed — the graph already encodes this relationship.

### Analysis Results

Analysis results are stored alongside the hypergraph, keyed by the entity they describe and the analysis that produced them.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub id: AnalysisResultId,
    pub node_id: NodeId,
    pub kind: AnalysisKind,
    /// Structured result data (JSON)
    pub data: Value,
    /// Hash of the inputs that produced this result (for invalidation)
    pub input_hash: u64,
    pub computed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisKind {
    // Behavioral
    ChangeFrequency,
    ChurnVelocity,
    ContributorConcentration,
    DocumentationCoverage,
    DocumentationFreshness,
    PromptHotspot,
    CorrectionHotspot,
    
    // Centrality
    PageRank,
    BetweennessCentrality,
    HITSScore,
    CompositeSalience,
    
    // Community
    CommunityAssignment,
    
    // Temporal
    CentralityTrend,
    ArchitecturalDrift,
    StabilityClassification,
    
    // Convention
    NamingPattern,
    TestingPattern,
    ErrorHandlingPattern,
    DocumentationStylePattern,
    AgentRuleValidation,
    
    // Task Pattern (from prompt mining)
    TaskPattern,
    DomainVocabulary,
    
    // Semantic (LLM)
    SemanticSummary,
    DesignRationale,
    InvariantDescription,
}
```

---

## SQLite Schema

### Core Tables

```sql
-- Schema version tracking
CREATE TABLE homer_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- All nodes in the hypergraph
CREATE TABLE nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,           -- NodeKind enum as string
    name TEXT NOT NULL,           -- Canonical name
    content_hash INTEGER,        -- For change detection (nullable)
    last_extracted TEXT NOT NULL, -- ISO 8601 timestamp
    metadata TEXT DEFAULT '{}',  -- JSON object
    
    UNIQUE(kind, name)           -- No duplicate nodes of same kind+name
);
CREATE INDEX idx_nodes_kind ON nodes(kind);
CREATE INDEX idx_nodes_name ON nodes(name);

-- Hyperedges (the relationships)
CREATE TABLE hyperedges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,           -- HyperedgeKind enum as string
    confidence REAL DEFAULT 1.0,
    last_updated TEXT NOT NULL,   -- ISO 8601 timestamp
    metadata TEXT DEFAULT '{}'    -- JSON object
);
CREATE INDEX idx_hyperedges_kind ON hyperedges(kind);

-- Membership in hyperedges (junction table)
CREATE TABLE hyperedge_members (
    hyperedge_id INTEGER NOT NULL REFERENCES hyperedges(id) ON DELETE CASCADE,
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0,
    
    PRIMARY KEY (hyperedge_id, node_id, role)
);
CREATE INDEX idx_hem_node ON hyperedge_members(node_id);
CREATE INDEX idx_hem_edge ON hyperedge_members(hyperedge_id);

-- Analysis results (derived data)
CREATE TABLE analysis_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,           -- AnalysisKind enum as string
    data TEXT NOT NULL,           -- JSON
    input_hash INTEGER NOT NULL,  -- For invalidation
    computed_at TEXT NOT NULL,    -- ISO 8601 timestamp
    
    UNIQUE(node_id, kind)        -- One result per (node, analysis_kind)
);
CREATE INDEX idx_ar_node ON analysis_results(node_id);
CREATE INDEX idx_ar_kind ON analysis_results(kind);

-- Full-text search index over text content
CREATE VIRTUAL TABLE text_search USING fts5(
    node_id,                     -- Reference back to nodes table
    content_type,                -- 'commit_message', 'pr_description', 'issue_body', 
                                 -- 'summary', 'doc_comment', 'document_body', 'prompt_text'
    content,                     -- The searchable text
    tokenize='porter unicode61'  -- Stemming + unicode support
);

-- Incrementality checkpoints
CREATE TABLE checkpoints (
    kind TEXT PRIMARY KEY,        -- 'git_sha', 'github_pr', 'github_issue', 'graph_snapshot',
                                 --  'document_scan', 'prompt_claude', 'prompt_cursor', etc.
    value TEXT NOT NULL,          -- The checkpoint value
    updated_at TEXT NOT NULL      -- ISO 8601 timestamp
);

-- Graph snapshots for temporal analysis
CREATE TABLE graph_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,          -- Release tag or timestamp
    snapshot_at TEXT NOT NULL,    -- ISO 8601 timestamp
    -- Snapshot data is the set of edges at this point
    -- Stored as a reference: "all hyperedges with last_updated <= snapshot_at"
    edge_count INTEGER NOT NULL,
    node_count INTEGER NOT NULL
);
```

### Projected Views

For common query patterns, define views:

```sql
-- Call graph as simple (caller, callee) pairs
CREATE VIEW call_graph AS
SELECT 
    caller.node_id AS caller_id,
    callee.node_id AS callee_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members caller ON e.id = caller.hyperedge_id AND caller.role = 'caller'
JOIN hyperedge_members callee ON e.id = callee.hyperedge_id AND callee.role = 'callee'
WHERE e.kind = 'Calls';

-- Import graph as simple (importer, imported) pairs
CREATE VIEW import_graph AS
SELECT 
    src.node_id AS importer_id,
    tgt.node_id AS imported_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members src ON e.id = src.hyperedge_id AND src.role = 'importer'
JOIN hyperedge_members tgt ON e.id = tgt.hyperedge_id AND tgt.role = 'imported'
WHERE e.kind = 'Imports';

-- Files modified by each commit
CREATE VIEW commit_files AS
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
CREATE VIEW document_references AS
SELECT 
    d.node_id AS document_id,
    c.node_id AS code_entity_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members d ON e.id = d.hyperedge_id AND d.role = 'document'
JOIN hyperedge_members c ON e.id = c.hyperedge_id AND c.role = 'code_entity'
WHERE e.kind = 'Documents';

-- Prompt sessions and the files they modified
CREATE VIEW prompt_modifications AS
SELECT 
    p.node_id AS prompt_id,
    f.node_id AS file_id,
    e.confidence
FROM hyperedges e
JOIN hyperedge_members p ON e.id = p.hyperedge_id AND p.role = 'prompt'
JOIN hyperedge_members f ON e.id = f.hyperedge_id AND f.role = 'file'
WHERE e.kind = 'PromptModifiedFiles';
```

---

## Store Trait

The trait abstraction allows alternative implementations (see [EVOLUTION.md](EVOLUTION.md) for libSQL future path):

```rust
#[async_trait]
pub trait HomerStore: Send + Sync {
    // --- Node operations ---
    async fn upsert_node(&self, node: &Node) -> Result<NodeId>;
    async fn get_node(&self, id: NodeId) -> Result<Option<Node>>;
    async fn get_node_by_name(&self, kind: NodeKind, name: &str) -> Result<Option<Node>>;
    async fn find_nodes(&self, filter: &NodeFilter) -> Result<Vec<Node>>;
    async fn mark_node_stale(&self, id: NodeId) -> Result<()>;
    async fn delete_stale_nodes(&self, older_than: DateTime<Utc>) -> Result<u64>;
    
    // --- Hyperedge operations ---
    async fn upsert_hyperedge(&self, edge: &Hyperedge) -> Result<HyperedgeId>;
    async fn get_edges_involving(&self, node_id: NodeId) -> Result<Vec<Hyperedge>>;
    async fn get_edges_by_kind(&self, kind: HyperedgeKind) -> Result<Vec<Hyperedge>>;
    async fn get_co_members(&self, node_id: NodeId, edge_kind: HyperedgeKind) -> Result<Vec<NodeId>>;
    
    // --- Analysis results ---
    async fn store_analysis(&self, result: &AnalysisResult) -> Result<()>;
    async fn get_analysis(&self, node_id: NodeId, kind: AnalysisKind) -> Result<Option<AnalysisResult>>;
    async fn get_analyses_by_kind(&self, kind: AnalysisKind) -> Result<Vec<AnalysisResult>>;
    async fn invalidate_analyses(&self, node_id: NodeId) -> Result<u64>;
    
    // --- Graph loading (for in-memory analysis) ---
    async fn load_call_graph(&self, filter: &SubgraphFilter) -> Result<InMemoryGraph>;
    async fn load_import_graph(&self, filter: &SubgraphFilter) -> Result<InMemoryGraph>;
    
    // --- Full-text search ---
    async fn index_text(&self, node_id: NodeId, content_type: &str, content: &str) -> Result<()>;
    async fn search_text(&self, query: &str, scope: SearchScope) -> Result<Vec<SearchHit>>;
    
    // --- Checkpoints ---
    async fn get_checkpoint(&self, kind: &str) -> Result<Option<String>>;
    async fn set_checkpoint(&self, kind: &str, value: &str) -> Result<()>;
    
    // --- Graph snapshots ---
    async fn create_snapshot(&self, label: &str) -> Result<SnapshotId>;
    async fn get_snapshot_diff(&self, from: SnapshotId, to: SnapshotId) -> Result<GraphDiff>;
    
    // --- Bulk operations ---
    async fn transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&dyn HomerStoreTx) -> Result<R> + Send;
    
    // --- Metrics ---
    async fn stats(&self) -> Result<StoreStats>;
}
```

### SubgraphFilter

```rust
pub enum SubgraphFilter {
    /// Load the full graph
    Full,
    /// N-hop neighborhood of specific nodes
    Neighborhood { centers: Vec<NodeId>, hops: u32 },
    /// Only nodes with composite salience above threshold
    HighSalience { min_score: f64 },
    /// Only nodes within a directory prefix
    Module { path_prefix: String },
    /// Only nodes of specific kinds
    OfKind { kinds: Vec<NodeKind> },
    /// Intersection of multiple filters
    And(Vec<SubgraphFilter>),
}
```

### InMemoryGraph

```rust
/// A graph loaded into memory for algorithm execution.
/// Uses petgraph internally.
pub struct InMemoryGraph {
    /// petgraph directed graph
    pub graph: DiGraph<NodeId, EdgeWeight>,
    /// NodeId → petgraph NodeIndex mapping
    pub node_map: HashMap<NodeId, NodeIndex>,
    /// Reverse mapping
    pub index_map: HashMap<NodeIndex, NodeId>,
}

pub struct EdgeWeight {
    pub confidence: f64,
    pub edge_kind: HyperedgeKind,
}
```

---

## Incrementality Protocol

### Content-Hash-Based Invalidation

Every node has an optional `content_hash`. When extraction runs:

1. Compute hash of current content (source code for files, message for commits, body for documents)
2. Compare against stored hash
3. If different: update node, invalidate downstream analysis results
4. If same: skip (node hasn't changed)

### Invalidation Cascade

When a node is updated, its analysis results become invalid. But analysis results for *other* nodes that depend on this node may also be invalid:

```
File F changed
  → F's ChangeFrequency: invalid (direct)
  → F's SemanticSummary: invalid (direct)
  → F's DocumentationCoverage: invalid (doc comment may have changed)
  → F's PageRank: invalid (call graph topology may have changed)
  → ALL PageRank scores: invalid (PageRank is a global metric)
  → Nodes calling F: their SemanticSummary may be invalid
    (but only if F's interface changed, not just implementation)
  → Documents referencing F: their cross-references may need re-resolution

Document D changed
  → D's content_hash: updated
  → D's Documents edges: re-resolve cross-references
  → Referenced code entities: DocumentationCoverage may change

AgentRule R changed
  → R's content_hash: updated
  → AgentRuleValidation: invalid (stated conventions may have changed)
```

**Practical approach**: Use coarse-grained invalidation initially. If *any* call graph edge changed, recompute *all* centrality metrics. This is sound (never stale) though not minimal. Refine to finer-grained invalidation as profiling reveals bottlenecks.

```rust
pub struct InvalidationPolicy {
    /// If true, any graph topology change invalidates all centrality scores
    pub global_centrality_on_topology_change: bool,
    /// If true, only invalidate semantic summaries when content_hash changes
    /// (not when neighborhood changes)
    pub conservative_semantic_invalidation: bool,
}
```

### Checkpoint Protocol

Each extractor maintains its own checkpoint:

| Extractor | Checkpoint Key | Checkpoint Value |
|-----------|---------------|-----------------|
| Git | `git_last_sha` | SHA of last processed commit |
| GitHub PRs | `github_last_pr` | Number of last processed PR |
| GitHub Issues | `github_last_issue` | Number of last processed issue |
| Graph | `graph_head_sha` | SHA at which current graph was built |
| Documents | `document_scan` | Timestamp of last document scan |
| Prompts (per source) | `prompt_{source}` | Timestamp of last processed interaction per source |

On `homer update`:
1. Read checkpoint for each extractor
2. Extract only data newer than checkpoint
3. Process extracted data through analyzers
4. Update checkpoints

---

## Database File Location

Default: `.homer/homer.db` in the repository root.

Configurable via:
- `--db-path` CLI flag
- `HOMER_DB_PATH` environment variable
- `[homer] db_path` in config

The `.homer/` directory also stores:
- `config.toml` — configuration
- `cache/` — LLM response cache (separate from main DB for easy clearing)
- `snapshots/` — exported graph snapshots (optional)
