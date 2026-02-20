# Concepts

This document explains how Homer works: its data model, pipeline stages, algorithms, and the insights they produce.

## The Hypergraph Model

Homer stores all extracted data in a **hypergraph** — a graph where edges can connect more than two nodes. This is stored in a SQLite database.

### Nodes

Nodes represent entities in the repository. Each has a `kind`, a unique `name`, and optional metadata. There are 15 node kinds:

| Kind | Example | Extracted From |
|------|---------|---------------|
| File | `src/store/sqlite.rs` | File tree |
| Function | `SqliteStore::upsert_node` | Tree-sitter parsing |
| Type | `struct HomerConfig` | Tree-sitter parsing |
| Module | `src/store/` | Directory structure |
| Commit | `abc123f` | Git history |
| PullRequest | `#42: Add auth middleware` | GitHub/GitLab API |
| Issue | `#15: Login fails on Safari` | GitHub/GitLab API |
| Contributor | `alice@example.com` | Git history |
| Release | `v1.0.0` | Git tags |
| Concept | `authentication` | LLM semantic analysis |
| ExternalDep | `serde 1.0` | Manifest parsing |
| Document | `README.md` | Document extractor |
| Prompt | `claude-code session abc` | Prompt extractor |
| AgentRule | `.claude/rules/auth.md` | Prompt extractor |
| AgentSession | `session-hash-xyz` | Prompt extractor |

### Hyperedges

Hyperedges represent relationships. Unlike normal graph edges, a hyperedge connects **N members**, each with a role. This naturally represents concepts like "commit C modified files {F1, F2, F3}" without artificial decomposition. There are 17 edge kinds:

| Kind | Members | Meaning |
|------|---------|---------|
| Modifies | commit (source), file (target) | Commit changed this file |
| Authored | contributor (source), commit (target) | Person authored this commit |
| Calls | function (caller), function (callee) | Function calls another function |
| Imports | file (source), file (target) | File imports from another file |
| Inherits | type (child), type (parent) | Type extends or implements another |
| Resolves | PR (source), issue (target) | Pull request resolves an issue |
| Reviewed | contributor (source), PR (target) | Person reviewed a pull request |
| Includes | module (parent), file (child) | Module includes a file |
| BelongsTo | file (child), module (parent) | File is part of this module |
| DependsOn | module (source), dep (target) | Project depends on external package |
| Aliases | node, node | Two names for the same entity |
| Documents | document (source), entity (target) | Document references this entity |
| PromptReferences | prompt (source), entity (target) | AI prompt referenced this entity |
| PromptModifiedFiles | prompt (source), file (target) | AI prompt led to modifying this file |
| RelatedPrompts | prompt, prompt | Related AI interaction sessions |
| CoChanges | file, file, ... | Files that change together |
| ClusterMembers | file, file, ... | Files in the same community cluster |
| Encompasses | concept (parent), entity (child) | Concept groups related entities |

### Analysis Results

Analysis results are attached to nodes. Each has a `kind` and a JSON `data` payload. There are 25 analysis kinds across 7 analyzers:

**Behavioral Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| ChangeFrequency | File | Total commits, 30/90/365-day counts |
| ChurnVelocity | File | Lines added/removed trend, acceleration |
| ContributorConcentration | File | Bus factor, contributor list |
| DocumentationCoverage | File | Whether file has doc comments, external docs |
| DocumentationFreshness | File | How recently documentation was updated |
| PromptHotspot | File | Frequency of AI agent interactions |
| CorrectionHotspot | File | Frequency of fix-after-AI-change patterns |

**Centrality Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| PageRank | File | PageRank score, rank position |
| BetweennessCentrality | File | Betweenness score (bridge importance) |
| HITSScore | File | Hub score, authority score |
| CompositeSalience | File | Combined score, classification |

**Community Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| CommunityAssignment | File | Community ID, directory alignment |

**Temporal Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| CentralityTrend | File | How centrality has changed across snapshots |
| ArchitecturalDrift | File | Whether file's role is shifting over time |
| StabilityClassification | File | Stability category (see below) |

**Convention Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| NamingPattern | File/Module | Detected naming conventions (snake_case, camelCase, etc.) |
| TestingPattern | File/Module | Testing approach patterns (unit, integration, property) |
| ErrorHandlingPattern | File/Module | Error handling style (Result, exceptions, panics) |
| DocumentationStylePattern | File/Module | Documentation conventions (doc comments, README, ADR) |
| AgentRuleValidation | File | Whether file follows agent rule constraints |

**Task Pattern Analyzer:**

| Kind | Attached To | Contains |
|------|------------|----------|
| TaskPattern | File/Module | Common task patterns (bug fix, feature, refactor) |
| DomainVocabulary | Module | Domain-specific terminology used in the codebase |

**Semantic Analyzer (LLM-powered):**

| Kind | Attached To | Contains |
|------|------------|----------|
| SemanticSummary | File/Function | LLM-generated summary of purpose and behavior |
| DesignRationale | File | Why the code is structured this way |
| InvariantDescription | Function/Type | Invariants that callers must respect |

## Salience and Stability

### Salience Classification

Files are classified by their combination of graph centrality and change activity:

| Classification | High Centrality? | High Churn? | Meaning |
|---------------|-----------------|-------------|---------|
| ActiveHotspot | Yes | Yes | Critical and actively developed — high impact, high activity |
| FoundationalStable | Yes | No | Core infrastructure, rarely changed — the most important to understand |
| PeripheralActive | No | Yes | Frequently changed but not structurally central |
| QuietLeaf | No | No | Low-impact, stable files |

The most interesting category is **FoundationalStable** — these are the files that matter most for an agent to understand but that are invisible to pure behavioral analysis.

### Stability Classification

A separate axis classifies files by their stability profile:

| Classification | Centrality | Change Rate | Meaning |
|---------------|-----------|-------------|---------|
| StableCore | High | Low | Rarely changes, high centrality — load-bearing infrastructure |
| ActiveCore | High | High | Frequently changes, high centrality — active development on critical code |
| StableLeaf | Low | Low | Rarely changes, low centrality — quiet utility code |
| ActiveLeaf | Low | High | Frequently changes, low centrality — active but peripheral |

## Pipeline

Homer's pipeline runs in four stages. Each stage is independent and fault-tolerant — individual failures are collected as warnings without aborting the pipeline.

### Stage 1: Extract

Extractors pull raw data from the repository and populate the hypergraph. Homer has 7 extractors:

**Git Extractor** — Walks commit history using `gix` (pure Rust git implementation). Creates Commit, Contributor, and Release nodes. Creates Modifies and Authored edges. Tracks `git_last_sha` checkpoint for incremental updates. Handles rename detection via `gix`'s `diff::tree_with_rewrites`.

**Structure Extractor** — Walks the file tree. Creates File and Module nodes. Creates BelongsTo edges. Parses manifests (Cargo.toml, package.json, pyproject.toml, go.mod) to create ExternalDep nodes and DependsOn edges. Respects include/exclude patterns from configuration.

**Graph Extractor** — Parses source files with tree-sitter via the `homer-graphs` crate. Creates Function and Type nodes. Creates Calls and Imports edges. Each language has a dedicated extractor that constructs scope graphs for precise symbol resolution. Import edges are resolved to actual file nodes where possible (e.g., Rust `crate::` and `super::` paths).

**Document Extractor** — Scans for documentation files (README, ADRs, doc directories). Creates Document nodes with metadata (title, sections, word count). Creates Documents edges linking docs to referenced source files.

**GitHub Extractor** — Fetches pull requests and issues via the GitHub API. Creates PullRequest and Issue nodes. Creates Resolves edges (PR → issue) and Reviewed edges (contributor → PR). Requires `GITHUB_TOKEN`. Depth-gated: skipped at `shallow`, limited at `standard`.

**GitLab Extractor** — Equivalent to the GitHub extractor for GitLab-hosted repositories. Fetches merge requests and issues. Requires `GITLAB_TOKEN`.

**Prompt Extractor** — Mines AI agent interactions (Claude Code sessions, `.claude/` rule files). Creates Prompt, AgentRule, and AgentSession nodes. Creates PromptReferences and PromptModifiedFiles edges. **Disabled by default** (`extraction.prompts.enabled = false`).

### Stage 2: Auto Snapshots

After extraction captures new state, Homer creates graph snapshots based on `[graph.snapshots]` configuration:

- **Release snapshots** — One snapshot per Release node (tagged version), labeled with the tag name (e.g., `v1.0.0`)
- **Commit-count snapshots** — One snapshot every N commits, labeled `auto-N` (e.g., `auto-100`, `auto-200`)

Snapshots are idempotent — if a snapshot with the same label already exists, it is not recreated.

### Stage 3: Analyze

Analyzers read from the hypergraph, compute derived insights, and write analysis results back. Homer has 7 analyzers, run in topological order based on their `produces()`/`requires()` declarations:

**Behavioral Analyzer** — Computes per-file metrics from git history:
- *Change Frequency* — How often each file was modified, with 30/90/365-day windows
- *Churn Velocity* — Rate of change (lines added + removed) over time
- *Contributor Concentration* — Bus factor: how many people have worked on each file
- *Co-Change Sets* — Groups of files that tend to change together (seed-and-grow algorithm)
- *Documentation Coverage/Freshness* — Whether files have docs and how current they are
- *Prompt/Correction Hotspots* — Files frequently touched by AI agents or corrected after AI changes

**Centrality Analyzer** — Loads the import graph into memory (via `petgraph`) and computes:
- *PageRank* — Importance based on how many files import a file, weighted by the importance of the importers (eigenvector centrality)
- *Betweenness Centrality* — Bridge importance: files that sit on the shortest paths between many other files (Brandes algorithm, k-source approximation for large graphs)
- *HITS* — Hub/authority scores: hubs import many files, authorities are imported by many files
- *Composite Salience* — Weighted combination of all centrality and behavioral signals into a single score

**Community Analyzer** — Runs the Louvain algorithm on the import graph to detect communities of structurally coupled files. Checks whether communities align with directory structure.

**Temporal Analyzer** — Analyzes how metrics change over time using snapshots:
- *Centrality Trend* — Whether a file is becoming more or less central
- *Architectural Drift* — Whether a file's structural role is shifting
- *Stability Classification* — Categorizes files as StableCore, ActiveCore, StableLeaf, ActiveLeaf

**Convention Analyzer** — Detects project conventions by analyzing patterns across files:
- *Naming patterns* (snake_case, camelCase, etc.)
- *Testing patterns* (unit, integration, property-based)
- *Error handling patterns* (Result types, exceptions, panics)
- *Documentation style* (doc comments, README, ADR)
- *Agent rule validation* (compliance with `.claude/rules/`)

**Task Pattern Analyzer** — Identifies recurring development patterns:
- *Task patterns* — Common commit patterns (bug fix, feature add, refactor)
- *Domain vocabulary* — Terms and concepts specific to the project

**Semantic Analyzer** (LLM-powered) — Uses an LLM to generate deep understanding:
- *Semantic summaries* — What a file or function actually does
- *Design rationale* — Why the code is structured this way
- *Invariant descriptions* — Constraints callers must respect

Gated by `llm.enabled = true` and `analysis.depth != shallow`. Only processes entities above `analysis.llm_salience_threshold`.

### Stage 4: Render

Renderers read from the hypergraph (both raw data and analysis results) and produce output files. Homer has 6 renderers:

**AGENTS.md Renderer** — Generates a structured context file for AI coding agents. Includes build commands (from CI config), module map, co-change patterns, danger zones (high churn + low bus factor), and conventions. Supports `<!-- homer:preserve -->` markers so human-curated sections are preserved during updates.

**Module Context Renderer** — Generates per-directory `.context.md` files with scoped information about each module: its purpose, key files, metrics summary.

**Risk Map Renderer** — Generates `homer-risk.json` with per-file risk factors in a machine-readable format for CI pipelines or agent guardrails.

**Skills Renderer** — Generates Claude Code skill files (`.claude/skills/*.md`) that encode domain knowledge as reusable skills.

**Topos Spec Renderer** — Generates topological specification files (`spec/*.toml`) capturing the structural relationships of the codebase in a formal format.

**Report Renderer** — Generates a human-readable analysis report (`homer-report.html` or Markdown) summarizing the full analysis.

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

Change frequency is normalized from its percentile range (0–100) to 0–1 before inclusion in the composite score.

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

### Topological Sorting of Analyzers

Analyzers declare what they `produce()` and `require()`. Homer uses Kahn's algorithm to sort them into a valid execution order:

1. Build a producer map: which analyzer produces each `AnalysisKind`
2. Build a dependency graph from `requires()` declarations
3. Run Kahn's algorithm (BFS from zero-in-degree nodes)
4. If cycles exist (should never happen), append remaining analyzers in original order

The current execution order is: behavioral → centrality → community → temporal → convention → task pattern → semantic (if enabled).

## Incrementality

Homer is designed for incremental updates:

- **Git extractor** tracks a `git_last_sha` checkpoint. On update, it only processes commits after the checkpoint.
- **Structure/document/prompt extractors** track checkpoint keys (`*_last_sha`) and skip when unchanged.
- **Graph extractor** tracks `graph_last_sha` and scopes extraction to files changed since that checkpoint.
- **Hyperedges** use deterministic semantic identity keys, so repeated equivalent writes are idempotent (no duplicate growth).
- **Analyzers** check `needs_rerun()` to decide whether to recompute. The `--force-analysis` flag clears cached results explicitly. `--force-semantic` clears only LLM-derived results.
- **Invalidation policy** controls how aggressively results are recomputed (see `[analysis.invalidation]`).

## Data Storage

All data is stored in a single SQLite database (`.homer/homer.db`) using WAL mode for concurrent read/write performance. The schema includes:

- `nodes` — All graph nodes with kind, name, metadata (JSON), content hash
- `hyperedges` — All relationships with kind, confidence, and deterministic identity key
- `hyperedge_members` — N-ary membership (node_id, role, position)
- `analysis_results` — Computed metrics (kind, node_id, data as JSON)
- `checkpoints` — Incrementality state (key-value pairs)
- `snapshots` / `snapshot_nodes` / `snapshot_edges` — Graph state at labeled points in time
- `nodes_fts` — Full-text search index on node names

Content hashes are stored as `u64` in Rust and cast to `i64` for SQLite via bit reinterpretation — a detail that matters only if you're querying the database directly.

The database is portable — copy `.homer/homer.db` to share the knowledge base. Regenerate it with `homer init` if needed.

## Next Steps

- [CLI Reference](cli-reference.md) — Complete command reference
- [Configuration](configuration.md) — Customize Homer's behavior
- [Getting Started](getting-started.md) — Hands-on guide
- [Internals](internals.md) — Architecture deep dive
- [Troubleshooting](troubleshooting.md) — Common issues
