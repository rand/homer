# Concepts

This document explains how Homer works: its data model, pipeline stages, algorithms, and the insights they produce.

## The Hypergraph Model

Homer stores all extracted data in a **hypergraph** — a graph where edges can connect more than two nodes. This is stored in a SQLite database.

### Nodes

Nodes represent entities in the repository. Each has a `kind`, a unique `name`, and optional metadata.

| Kind | Example | Extracted From |
|------|---------|---------------|
| File | `src/store/sqlite.rs` | File tree |
| Function | `SqliteStore::upsert_node` | Tree-sitter parsing |
| Type | `struct HomerConfig` | Tree-sitter parsing |
| Module | `src/store/` | Directory structure |
| Commit | `abc123f` | Git history |
| Contributor | `alice@example.com` | Git history |
| Document | `README.md` | Document extractor |
| ExternalDep | `serde 1.0` | Manifest parsing |
| Release | `v1.0.0` | Git tags |

### Hyperedges

Hyperedges represent relationships. Unlike normal graph edges, a hyperedge connects **N members**, each with a role. This naturally represents concepts like "commit C modified files {F1, F2, F3}" without artificial decomposition.

| Kind | Members | Meaning |
|------|---------|---------|
| Modifies | commit (source), file (target) | Commit changed this file |
| Authored | contributor (source), commit (target) | Person authored this commit |
| Calls | function (caller), function (callee) | Function calls another function |
| Imports | file (source), file (target) | File imports from another file |
| BelongsTo | file (child), module (parent) | File is part of this module |
| DependsOn | module (source), dep (target) | Project depends on external package |
| CoChanges | file, file, ... | Files that change together |
| Documents | document (source), entity (target) | Document references this entity |

### Analysis Results

Analysis results are attached to nodes. Each has a `kind` and a JSON `data` payload.

| Kind | Attached To | Contains |
|------|------------|----------|
| ChangeFrequency | File | Total commits, 30/90/365-day counts |
| ChurnVelocity | File | Lines added/removed trend, acceleration |
| ContributorConcentration | File | Bus factor, contributor list |
| CoChangeSet | File | Co-changing files with confidence scores |
| PageRank | File | PageRank score, rank position |
| BetweennessCentrality | File | Betweenness score (bridge importance) |
| HITSScore | File | Hub score, authority score |
| CommunityAssignment | File | Community ID, directory alignment |
| CompositeSalience | File | Combined score, classification |
| StabilityClassification | File | Stability category |

## Pipeline

Homer's pipeline runs in three stages. Each stage is independent and fault-tolerant — individual failures are collected as warnings without aborting the pipeline.

### Stage 1: Extract

Extractors pull raw data from the repository and populate the hypergraph.

**Git Extractor** — Walks commit history using `gix` (pure Rust git implementation). Creates Commit, Contributor, and Release nodes. Creates Modifies and Authored edges. Tracks `git_last_sha` checkpoint for incremental updates. Handles rename detection via `gix`'s `diff::tree_with_rewrites`.

**Structure Extractor** — Walks the file tree. Creates File and Module nodes. Creates BelongsTo edges. Parses manifests (Cargo.toml, package.json, pyproject.toml, go.mod) to create ExternalDep nodes and DependsOn edges. Respects include/exclude patterns from configuration.

**Graph Extractor** — Parses source files with tree-sitter via the `homer-graphs` crate. Creates Function and Type nodes. Creates Calls and Imports edges. Each language has a dedicated extractor that uses tree-sitter queries to find function definitions, call sites, imports, and doc comments. Import edges are resolved to actual file nodes where possible (e.g., Rust `crate::` and `super::` paths).

**Document Extractor** — Scans for documentation files (README, ADRs, doc directories). Creates Document nodes with metadata (title, sections, word count). Creates Documents edges linking docs to referenced source files.

### Stage 2: Analyze

Analyzers read from the hypergraph, compute derived insights, and write analysis results back.

**Behavioral Analyzer** — Computes per-file metrics from git history:
- *Change Frequency* — How often each file was modified, with 30/90/365-day windows
- *Churn Velocity* — Rate of change (lines added + removed) over time
- *Contributor Concentration* — Bus factor: how many people have worked on each file
- *Co-Change Sets* — Groups of files that tend to change together (seed-and-grow algorithm)
- *Stability Classification* — Categorizes files as StableCore, ActiveDevelopment, Hotspot, Legacy, etc.

**Centrality Analyzer** — Loads the import graph into memory (via `petgraph`) and computes:
- *PageRank* — Importance based on how many files import a file, weighted by the importance of the importers (eigenvector centrality)
- *Betweenness Centrality* — Bridge importance: files that sit on the shortest paths between many other files (Brandes algorithm, k-source approximation for large graphs)
- *HITS* — Hub/authority scores: hubs import many files, authorities are imported by many files

**Community Detector** — Runs the Louvain algorithm on the import graph to detect communities of structurally coupled files. Checks whether communities align with directory structure (directory-aligned communities suggest good modular design; misaligned communities suggest cross-cutting concerns).

**Composite Salience** — Combines all signals into a single score per file:
- Weighted combination of PageRank, betweenness, HITS authority, change frequency, and bus factor risk
- Classifies each file: FoundationalStable (high centrality, low churn), ActiveHub (high centrality, high churn), VolatilePeripheral, etc.
- The key insight: files with high graph centrality but low change frequency are "quiescent high-centrality" nodes — they're critical infrastructure that behavioral analysis alone would miss

### Stage 3: Render

Renderers read from the hypergraph (both raw data and analysis results) and produce output files.

**AGENTS.md Renderer** — Generates a structured context file for AI coding agents. Includes build commands (from CI config), module map, co-change patterns, danger zones (high churn + low bus factor), and conventions. Supports `<!-- homer:preserve -->` markers so human-curated sections are preserved during updates.

**Module Context Renderer** — Generates per-directory `.context.md` files with scoped information about each module: its purpose, key files, metrics summary.

**Risk Map Renderer** — Generates `homer-risk.json` with per-file risk factors in a machine-readable format for CI pipelines or agent guardrails.

## Key Algorithms

### Composite Salience

The composite salience score combines behavioral and structural signals:

```
salience = w_pr * pagerank
         + w_bt * betweenness
         + w_hits * hits_authority
         + w_churn * normalized_churn
         + w_bus * (1 - normalized_bus_factor)
```

Files are classified into categories based on their salience profile:

| Classification | High Centrality? | High Churn? | Meaning |
|---------------|-----------------|-------------|---------|
| FoundationalStable | Yes | No | Core infrastructure, rarely changed |
| ActiveHub | Yes | Yes | Critical and actively developed |
| VolatilePeripheral | No | Yes | Frequently changed but not central |
| Stable | No | No | Quiet, low-impact files |

The most interesting category is **FoundationalStable** — these are the files that matter most for an agent to understand but that are invisible to pure behavioral analysis.

### Co-Change Detection

Homer detects co-change sets using a seed-and-grow algorithm:

1. For each file, collect all commits that touched it
2. For each pair of files, compute the Jaccard similarity of their commit sets
3. Seed clusters from high-similarity pairs (> 0.5 confidence)
4. Grow clusters by adding files that co-change with most existing members
5. Filter to sets with >= 3 members and >= 0.3 average confidence

### Community Detection (Louvain)

The Louvain algorithm finds communities by optimizing modularity:

1. Start with each node in its own community
2. For each node, try moving it to each neighbor's community
3. Accept the move that gives the largest modularity gain
4. Repeat until no moves improve modularity
5. Collapse communities into super-nodes and repeat

Homer checks directory alignment: if most files in a community share a common directory prefix, the community is "directory-aligned." Misaligned communities reveal cross-cutting concerns that span directories.

### Betweenness Centrality (Brandes)

Betweenness measures how often a file sits on the shortest path between two other files in the import graph. High betweenness means the file is a "bridge" — removing it would disconnect parts of the codebase.

Homer uses the Brandes algorithm for efficient computation, with k-source approximation (sampling a subset of source nodes) for graphs larger than 50,000 nodes.

## Incrementality

Homer is designed for incremental updates:

- **Git extractor** tracks a `git_last_sha` checkpoint. On update, it only processes commits after the checkpoint.
- **Structure and graph extractors** use content-hash-based upsert semantics. Nodes and edges are identified by (kind, name) pairs. Re-extracting unchanged files is idempotent — no duplicates are created.
- **Analyzers** recompute all metrics on each run (analysis is fast relative to extraction). The `--force-analysis` flag clears cached results explicitly.

## Data Storage

All data is stored in a single SQLite database (`.homer/homer.db`) using WAL mode for concurrent read/write performance. The schema includes:

- `nodes` — All graph nodes with kind, name, metadata (JSON), content hash
- `hyperedges` — All relationships with kind and confidence score
- `hyperedge_members` — N-ary membership (node_id, role, position)
- `analysis_results` — Computed metrics (kind, node_id, data as JSON)
- `checkpoints` — Incrementality state (key-value pairs)
- `nodes_fts` — Full-text search index on node names

The database is portable — copy `.homer/homer.db` to share the knowledge base. Regenerate it with `homer init` if needed.

## Next Steps

- [Configuration](configuration.md) — Customize Homer's behavior
- [Getting Started](getting-started.md) — Hands-on guide
- [Troubleshooting](troubleshooting.md) — Common issues
