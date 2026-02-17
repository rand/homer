# Homer Analyzers

> Behavioral, centrality, temporal, semantic, convention, and task pattern analysis.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [STORE.md](STORE.md) · [GRAPH_ENGINE.md](GRAPH_ENGINE.md) · [RENDERERS.md](RENDERERS.md)  
**Prior art**: Adam Tornhill's [behavioral code analysis](https://pragprog.com/titles/atcrime2/your-code-as-a-crime-scene-second-edition/), [CodeScene](https://codescene.com/), NetworkX graph algorithms

---

## Analyzer Trait

```rust
#[async_trait]
pub trait Analyzer: Send + Sync {
    fn name(&self) -> &str;
    
    /// What analysis kinds this analyzer produces
    fn produces(&self) -> Vec<AnalysisKind>;
    
    /// What data this analyzer requires from the store
    fn requires(&self) -> Vec<DataRequirement>;
    
    /// Run analysis, reading from and writing to the store
    async fn analyze(&self, store: &dyn HomerStore, config: &AnalyzeConfig) -> Result<AnalyzeStats>;
    
    /// Check if this analyzer needs to rerun (any inputs changed since last run?)
    async fn needs_rerun(&self, store: &dyn HomerStore) -> Result<bool>;
}
```

---

## Behavioral Analyzer

**Cost**: Low (pure computation over commit data)  
**Depends on**: Git history extraction, document extraction, prompt extraction  
**Produces**: `ChangeFrequency`, `ChurnVelocity`, `ContributorConcentration`, `DocumentationCoverage`, `DocumentationFreshness`, `PromptHotspot`, `CorrectionHotspot`, `CoChanges` hyperedges  
**Reference**: Tornhill, "Your Code as a Crime Scene" (2015)

### Change Frequency

For each file, count commits that modified it within configurable time windows:

```rust
pub struct ChangeFrequencyResult {
    /// Total commits modifying this file (all time)
    pub total_changes: u64,
    /// Changes in last 30/90/365 days
    pub changes_30d: u64,
    pub changes_90d: u64,
    pub changes_365d: u64,
    /// Rank among all files (0.0 = least changed, 1.0 = most changed)
    pub percentile: f64,
}
```

**Query**: Count `Modifies` hyperedges involving each file node, filtered by commit timestamp.

### Churn Velocity

Is a file's change rate accelerating or decelerating?

```rust
pub struct ChurnVelocityResult {
    /// Trend direction
    pub trend: ChurnTrend,  // Accelerating, Stable, Decelerating, Quiescent
    /// Slope of linear regression on monthly change counts
    pub slope: f64,
    /// Lines added - lines deleted over time (growing = feature work, shrinking = refactoring)
    pub net_growth_30d: i64,
    pub net_growth_90d: i64,
}
```

### Contributor Concentration (Bus Factor)

For each file, who has touched it and how much?

```rust
pub struct ContributorConcentrationResult {
    /// Number of unique contributors
    pub unique_contributors: u32,
    /// Bus factor: minimum contributors responsible for 80% of changes
    pub bus_factor: u32,
    /// Top contributors with their % of total changes
    pub top_contributors: Vec<(String, f64)>,
    /// Is this a knowledge silo? (bus_factor == 1)
    pub is_knowledge_silo: bool,
}
```

### Documentation Coverage

What percentage of high-salience entities have doc comments? This metric bridges the document extractor and the centrality analyzer.

```rust
pub struct DocumentationCoverageResult {
    /// Fraction of high-salience entities (above threshold) with doc comments
    pub high_salience_coverage: f64,
    /// Fraction of all public entities with doc comments
    pub public_coverage: f64,
    /// List of high-salience entities missing doc comments (risk flags)
    pub undocumented_critical: Vec<NodeId>,
}
```

### Documentation Freshness

Does the doc comment's content still match what the function actually does?

```rust
pub struct DocumentationFreshnessResult {
    /// Has the function's source changed since its doc comment was last updated?
    pub is_stale: bool,
    /// Number of source-changing commits since the doc comment was last modified
    pub commits_since_doc_update: u32,
    /// Staleness risk: stale doc on a high-salience entity = high risk
    pub staleness_risk: f64,
}
```

**Detection**: Compare the doc comment's content hash against the function's recent commit history. If the function body changed N times since the doc comment last changed, the doc may be stale.

### Prompt Hotspots

Files most frequently referenced in agent interactions — a signal of developer attention independent of commit frequency.

```rust
pub struct PromptHotspotResult {
    /// Number of prompts referencing this file
    pub prompt_references: u32,
    /// Number of agent sessions involving this file
    pub session_count: u32,
    /// Percentile among all files
    pub percentile: f64,
}
```

### Correction Hotspots

Files that get repeated corrections in agent interactions — a signal that the codebase is confusing to agents in this area.

```rust
pub struct CorrectionHotspotResult {
    /// Number of correction events involving this file
    pub correction_count: u32,
    /// Correction rate: corrections / total interactions for this file
    pub correction_rate: f64,
    /// Is this a persistent confusion zone? (correction_rate > threshold)
    pub is_confusion_zone: bool,
}
```

### Co-Change Detection

**The key behavioral insight**: Files that repeatedly appear in the same commits have an *implicit coupling* that may not be reflected in the import/call graph. This is architectural coupling that code structure doesn't declare.

**Algorithm**:

1. For each pair of files (A, B), count commits where both were modified
2. Compute support: `support(A,B) = count(A∩B) / total_commits`
3. Compute confidence: `confidence(A→B) = count(A∩B) / count(A)`
4. Filter: keep pairs where support > threshold AND confidence > threshold
5. **Extend to N-ary (seed-and-grow, not clique enumeration)**: Finding maximal cliques in the co-change graph is NP-hard and explodes on real repos. Instead, Homer uses a greedy seed-and-grow strategy:
   - Sort pairs by confidence (descending)
   - Pick the highest-confidence pair as a seed
   - Attempt to grow: for each candidate file C not in the group, add C if it co-changes with *every* member of the group above `min_confidence`
   - Stop growing when no candidate improves the group's minimum pairwise confidence by more than `min_marginal_gain` (default: 0.05)
   - Cap group arity at `max_group_size` (default: 8) — groups larger than this are almost always "everything changes together" noise
   - Mark consumed pairs and repeat until no ungrouped pairs remain above threshold
   - Pairs that don't grow beyond size 2 are emitted as binary co-change edges

This is intentionally not frequent-itemset mining. The goal is actionable change sets ("when you touch A, you probably need to touch B and C"), not exhaustive pattern discovery.

```rust
pub struct CoChangeConfig {
    /// Minimum support (fraction of total commits)
    pub min_support: f64,       // Default: 0.02 (2% of commits)
    /// Minimum confidence (fraction of file's commits)
    pub min_confidence: f64,    // Default: 0.3 (30% of times A changes, B also changes)
    /// Time window to consider (None = all history)
    pub window: Option<Duration>,
    /// Minimum commit count for pair to be considered
    pub min_co_occurrences: u32, // Default: 3
    /// Maximum arity of co-change hyperedges
    pub max_group_size: usize,   // Default: 8
    /// Minimum marginal gain to continue growing a group
    pub min_marginal_gain: f64,  // Default: 0.05
}
```

**Output**: `Hyperedge(CoChanges)` with confidence score and metadata including co-occurrence count and time span.

---

## Centrality Analyzer

**Cost**: Medium (graph algorithm computation)  
**Depends on**: Call graph and import graph from graph extractor  
**Produces**: `PageRank`, `BetweennessCentrality`, `HITSScore`, `CompositeSalience`  
**Library**: `petgraph` for graph data structures and algorithms

### Implementation Strategy

| Metric | Source | Scale Notes |
|--------|--------|-------------|
| PageRank | `petgraph::algo::page_rank` | O(E·k) where k=iterations. Handles 100k+ nodes fine. |
| Betweenness | Self-implemented (Brandes algorithm) | O(V·E). For graphs >50k nodes, switch to approximate betweenness (random k-source sampling, k=√V). |
| HITS | Self-implemented (power iteration) | O(E·k). Same iteration model as PageRank. |

Petgraph provides PageRank directly. Betweenness centrality and HITS are not in petgraph's standard algorithms — Homer implements them using petgraph's graph data structures (`DiGraph`) but custom algorithm code. Petgraph's `rayon` support applies to parallel iteration over node/edge collections, not to parallelizing the centrality algorithms themselves; the algorithms are inherently sequential per-source (betweenness) or per-iteration (HITS, PageRank).

**Approximation policy**: For graphs exceeding `approx_threshold` nodes (default: 50,000), betweenness centrality switches to k-source approximation. The approximation factor and threshold are configurable:

```rust
pub struct CentralityConfig {
    /// Node count above which betweenness uses k-source approximation
    pub approx_threshold: usize,  // Default: 50_000
    /// Number of source nodes for approximate betweenness (default: sqrt(V))
    pub approx_k: Option<usize>,
    /// PageRank damping factor
    pub damping: f64,             // Default: 0.85
    /// Convergence threshold for iterative algorithms
    pub convergence: f64,         // Default: 1e-6
    /// Max iterations for iterative algorithms
    pub max_iterations: u32,      // Default: 100
}
```

### PageRank

Applied to the **call graph**: identifies the most-depended-on functions. A function called by many other functions that are themselves called by many functions has high PageRank.

```rust
pub struct PageRankResult {
    pub score: f64,              // 0.0 - 1.0 (normalized)
    pub rank: u32,               // 1 = highest PageRank in the graph
    pub in_degree: u32,          // Number of functions calling this one
    pub out_degree: u32,         // Number of functions this one calls
    pub graph_tier: ResolutionTier, // Precision of underlying graph
}
```

**Parameters**: See `CentralityConfig` above (damping=0.85, convergence=1e-6, max_iterations=100).

**Confidence annotation**: PageRank scores computed from `Precise` tier graphs are more trustworthy than those from `Heuristic` tier. The `graph_tier` field lets consumers calibrate trust.

### Betweenness Centrality

Applied to the **import graph**: identifies modules that serve as bridges between subsystems. High betweenness = many shortest paths between other modules pass through this module.

```rust
pub struct BetweennessResult {
    pub score: f64,              // Normalized betweenness centrality
    pub rank: u32,
    /// Is this a "bridge" module? (betweenness >> average)
    pub is_bridge: bool,
}
```

**Interpretation**: A module with high betweenness is a structural bottleneck. If it breaks, it disconnects parts of the codebase. Agents should treat these modules with extra care.

### HITS (Hub/Authority)

Applied to the **call graph**: distinguishes orchestrator functions (hubs that call many things) from utility functions (authorities called by many things).

```rust
pub struct HITSResult {
    pub hub_score: f64,          // High = this function orchestrates many others
    pub authority_score: f64,    // High = this function is called by many orchestrators
    pub classification: HITSClass, // Hub, Authority, Both, Neither
}
```

**Interpretation for agents**:
- **Authorities** (high authority score): These are the primitives. Understand them deeply before using them. Changes here have wide blast radius.
- **Hubs** (high hub score): These are orchestrators. Understand the flow they implement. Changes here affect coordination patterns.

### Composite Salience Score

The core metric that gates everything else. Combines multiple signals into a single "how important is this entity?" score.

```rust
pub struct CompositeSalienceResult {
    pub score: f64,              // 0.0 - 1.0
    pub components: SalienceComponents,
    pub classification: SalienceClass,
}

pub struct SalienceComponents {
    pub pagerank: f64,           // Weight: 0.3
    pub betweenness: f64,        // Weight: 0.15
    pub authority: f64,          // Weight: 0.15
    pub change_frequency: f64,   // Weight: 0.15
    pub bus_factor_risk: f64,    // Weight: 0.1 (inverse: lower bus factor = higher risk)
    pub code_size: f64,          // Weight: 0.05 (larger = more to understand)
    pub test_presence: f64,      // Weight: 0.1 (absence of tests for high-centrality = risk)
}

pub enum SalienceClass {
    /// High centrality + high churn: hot and critical
    HotCritical,
    /// High centrality + low churn: foundational and stable (quiescent high-centrality)
    FoundationalStable,
    /// Low centrality + high churn: active but localized
    ActiveLocalized,
    /// Low centrality + low churn: background code
    Background,
    /// High centrality + low bus factor: knowledge-siloed critical code
    CriticalSilo,
}
```

**This is Homer's key differentiator.** The `FoundationalStable` classification is invisible to any tool that only looks at change frequency. These are the load-bearing walls — unchanged for months, depended on by everything, and absolutely critical for an agent to understand before touching.

---

## Community Detection

**Cost**: Medium  
**Depends on**: Import graph  
**Produces**: `CommunityAssignment`  
**Algorithm**: Louvain method (or Leiden for better quality, if a Rust implementation is available)

Community detection identifies clusters of modules that are more tightly connected to each other than to the rest of the codebase. These clusters represent the *actual* module boundaries, which may differ from the directory structure.

```rust
pub struct CommunityAssignmentResult {
    pub community_id: u32,
    pub community_label: Option<String>,  // LLM-generated label (if semantic analyzer ran)
    pub modularity_contribution: f64,     // How much this node contributes to community cohesion
    /// Does this node's directory path match its community peers?
    pub directory_aligned: bool,
}
```

**Divergence detection**: When community membership diverges from directory structure, it suggests architectural drift. If `src/auth/validate.rs` is in the same community as `src/payment/charge.rs` but not with `src/auth/session.rs`, there's an implicit coupling between auth-validation and payment that the directory structure doesn't reflect.

---

## Temporal Analyzer

**Cost**: Medium  
**Depends on**: Graph snapshots, centrality results over time  
**Produces**: `CentralityTrend`, `ArchitecturalDrift`, `StabilityClassification`

### Graph Snapshots

Homer captures graph snapshots at natural checkpoints:
- Every tagged release
- Every N commits (configurable, default 100)
- On demand (`homer snapshot create "label"`)

### Centrality Trend

For each node, track how its PageRank/betweenness changes across snapshots:

```rust
pub struct CentralityTrendResult {
    pub trend: TrendDirection,     // Rising, Stable, Falling
    pub slope: f64,                // Rate of change per snapshot
    pub current_score: f64,
    pub score_history: Vec<(String, f64)>,  // (snapshot_label, score)
}

pub enum TrendDirection {
    /// Centrality increasing: more things depend on this over time
    Rising,
    /// Centrality stable
    Stable,
    /// Centrality decreasing: dependencies being removed
    Falling,
}
```

**"Rising importance" nodes**: Centrality increasing over time even without the node itself being modified. This means other code is increasingly depending on it. Candidate for proactive hardening (more tests, documentation, review requirements).

### Architectural Drift

Measure how the graph's structure is changing:

```rust
pub struct ArchitecturalDriftResult {
    /// Are cross-community edges increasing? (coupling creep)
    pub cross_community_edge_trend: TrendDirection,
    /// Ratio of cross-community edges to total edges
    pub coupling_ratio: f64,
    pub coupling_ratio_history: Vec<(String, f64)>,
    /// Specific new cross-community edges introduced since last snapshot
    pub new_cross_boundary_imports: Vec<(NodeId, NodeId)>,
}
```

### Stability Classification

Combines centrality with temporal behavior:

```rust
pub enum StabilityClass {
    /// High centrality, low churn, no breaking changes: stable core
    /// Agent instruction: "conform to interface, do not modify without understanding all callers"
    StableCore,
    /// High centrality, actively evolving: critical moving part
    /// Agent instruction: "changes here have wide blast radius, full test suite required"
    ActiveCritical,
    /// Low centrality, stable: reliable background code
    /// Agent instruction: "safe to use, unlikely to change under you"
    ReliableBackground,
    /// High churn, unstable interface: volatile
    /// Agent instruction: "expect this to change, code defensively against it"
    Volatile,
    /// Centrality declining: being phased out
    /// Agent instruction: "consider migrating away from this"
    Declining,
}
```

---

## Convention Analyzer

**Cost**: Low-Medium (heuristic + optional LLM)  
**Depends on**: ASTs from tree-sitter, agent rule files from prompt extractor  
**Produces**: `NamingPattern`, `TestingPattern`, `ErrorHandlingPattern`, `DocumentationStylePattern`, `AgentRuleValidation`

### Naming Conventions

Scan identifiers across the codebase and detect modal patterns:

```rust
pub struct NamingPatternResult {
    pub convention: NamingConvention,  // snake_case, camelCase, PascalCase, etc.
    pub adherence_rate: f64,          // What % of identifiers follow this convention?
    pub common_prefixes: Vec<(String, u32)>,  // (prefix, count): "get_", "is_", "handle_"
    pub common_suffixes: Vec<(String, u32)>,  // (suffix, count): "_test", "Handler", "Service"
}
```

### Testing Patterns

```rust
pub struct TestingPatternResult {
    /// Test framework detected
    pub framework: Option<String>,    // "pytest", "jest", "cargo test", etc.
    /// Test file naming convention
    pub file_pattern: String,         // "*_test.rs", "test_*.py", "*.spec.ts"
    /// Test co-location (same directory) or separate tree?
    pub co_located: bool,
    /// Assertion style
    pub assertion_style: String,      // "assert!()", "expect().toBe()", etc.
    /// Mocking patterns
    pub mock_patterns: Vec<String>,
    /// Approximate test coverage (if CI config reveals coverage tool)
    pub coverage_tool: Option<String>,
}
```

### Error Handling Patterns

```rust
pub struct ErrorHandlingResult {
    /// Primary error handling approach
    pub approach: ErrorApproach,       // Result/Option, Exceptions, Error codes
    /// Custom error types used
    pub custom_error_types: Vec<String>,
    /// Error propagation pattern
    pub propagation: String,           // "? operator", "try/catch", "if err != nil"
}
```

### Documentation Style Patterns

Detect the project's documentation conventions from doc comments:

```rust
pub struct DocumentationStyleResult {
    /// Dominant doc style (rustdoc, jsdoc, numpy, etc.)
    pub style: DocStyle,
    /// Adherence rate among documented entities
    pub adherence_rate: f64,
    /// Whether the project documents parameters, return values, examples
    pub documents_params: bool,
    pub documents_returns: bool,
    pub documents_examples: bool,
}
```

These conventions go into AGENTS.md: "when adding a new public function, include a doc comment following [detected pattern]."

### Agent Rule Validation

Agent rule files (CLAUDE.md, .cursor/rules) are *explicitly stated conventions*. The convention analyzer validates these against actual code patterns:

```rust
pub struct AgentRuleValidationResult {
    /// Stated conventions that match actual patterns
    pub validated: Vec<ValidatedRule>,
    /// Stated conventions that don't match reality (drift)
    pub drifted: Vec<DriftedRule>,
    /// Actual patterns not mentioned in any agent rule file
    pub undocumented: Vec<UndocumentedPattern>,
}

pub struct DriftedRule {
    pub rule_source: String,        // "CLAUDE.md", ".cursor/rules/naming.mdc", etc.
    pub stated_convention: String,  // "always use snake_case"
    pub actual_pattern: String,     // "40% camelCase detected"
    pub adherence_rate: f64,
}
```

If CLAUDE.md says "always use snake_case" but the codebase has 40% camelCase, that's a drift signal.

---

## Task Pattern Analyzer

**Cost**: Medium (algorithmic + optional LLM for classification)  
**Depends on**: Prompt extraction data  
**Produces**: `TaskPattern`, `DomainVocabulary`

This analyzer extracts recurring task shapes from prompt history — the patterns that developers perform often enough to codify as skills.

### Task Pattern Extraction

```rust
pub struct PromptMetrics {
    /// Files most frequently referenced in prompts (developer attention signal)
    pub prompt_hotspots: Vec<(PathBuf, u32)>,
    /// Files that get repeated corrections (confusion signal)
    pub correction_hotspots: Vec<(PathBuf, u32)>,
    /// Most common task patterns extracted from prompts
    pub task_patterns: Vec<TaskPattern>,
    /// Domain vocabulary: terms developers use vs. code identifiers
    pub vocabulary_map: Vec<(String, Vec<String>)>,  // human term → code identifiers
}

pub struct TaskPattern {
    /// Inferred task type (e.g., "add API endpoint", "fix bug", "refactor")
    pub pattern_name: String,
    /// Files typically involved
    pub typical_files: Vec<PathBuf>,
    /// Frequency (how many prompts match this pattern)
    pub frequency: u32,
    /// Canonical example prompt
    pub example: String,
}
```

### Domain Vocabulary

Maps human-language terms to code identifiers. When developers say "the order pipeline," they mean `ProcessOrderWorkflow` in `src/workflows/orders.rs`. This mapping is extracted from prompt text and stored for rendering into AGENTS.md.

### Analysis Integration

**Prompt text provides natural-language descriptions of code areas** — LLM summarization can use these as "what the developer said this does" to cross-validate against "what the code actually does."

**Correction patterns identify where agent understanding breaks down** — these feed directly into risk maps as "agent confusion zones."

---

## Semantic Analyzer

**Cost**: HIGH (LLM API calls)  
**Depends on**: Composite salience scores (gates which nodes get summarized), doc comments from graph extraction  
**Produces**: `SemanticSummary`, `DesignRationale`, `InvariantDescription`

### Salience Gating

**The most important performance decision in Homer.** The semantic analyzer only runs on entities above a configurable salience threshold:

```rust
pub struct SemanticConfig {
    /// Minimum composite salience to trigger LLM summarization
    pub salience_threshold: f64,      // Default: 0.7
    /// Maximum entities to summarize per run
    pub max_entities_per_run: u32,    // Default: 50
    /// Maximum PR descriptions to analyze
    pub max_prs_to_analyze: u32,      // Default: 100
    /// LLM provider configuration
    pub provider: LlmProviderConfig,
    /// Model to use
    pub model: String,                // Default: "claude-sonnet-4-20250514"
}
```

On a repo with 10,000 functions, if salience threshold is 0.7, perhaps 200 functions qualify. Of those, maybe 50 are re-summarized on an incremental update (the others haven't changed). That's 50 LLM calls, not 10,000.

### Doc-Comment-Aware Summarization

For high-salience entities *with* good doc comments, the LLM call can be skipped or simplified — the doc comment *is* the human-authored summary. The LLM only needs to confirm or augment, not generate from scratch:

```rust
fn should_call_llm(node: &Node, salience: f64) -> LlmDecision {
    if salience < config.salience_threshold {
        return LlmDecision::Skip;  // Below threshold
    }
    
    match node.metadata.get("doc_comment") {
        Some(doc) if doc_is_substantial(doc) => {
            // Good doc comment exists — use it as the summary,
            // optionally ask LLM to verify/augment
            LlmDecision::AugmentDocComment
        }
        _ => {
            // No doc comment or it's trivial — full LLM summarization
            LlmDecision::FullSummarize
        }
    }
}
```

For high-salience entities *without* doc comments, this is a stronger signal for the risk map: "critical and undocumented."

### Entity Summarization

For each high-salience function/type/module that needs full LLM summarization:

**Prompt template**:
```
You are analyzing a code entity for a repository mining tool. 
Produce a concise summary optimized for AI coding agents.

Entity: {qualified_name}
Kind: {kind} (function/type/module)
File: {file_path}
Centrality: PageRank={pagerank}, Betweenness={betweenness}
Classification: {salience_class}
Called by: {callers_list} ({caller_count} total)
Calls: {callees_list} ({callee_count} total)
Doc comment: {doc_comment_if_present}

Source code:
```{language}
{source_code}
```

Produce a JSON response with:
1. "summary": 1-2 sentence description of what this entity does
2. "invariants": list of apparent invariants (things that must remain true)
3. "usage_pattern": how callers typically use this entity
4. "caution": any warnings for an agent that might modify this code
```

### Design Rationale Extraction

For high-salience PRs (those that touched high-salience code):

**Prompt template**:
```
This PR made significant changes to important code. 
Extract the design rationale.

PR #{number}: {title}
Description: {body}
Files changed: {files_list}
Review comments: {review_comments}

Produce a JSON response with:
1. "motivation": why this change was made
2. "approach": what approach was chosen
3. "alternatives_considered": any alternatives mentioned
4. "tradeoffs": explicit or implicit tradeoffs
```

### Caching and Invalidation

LLM responses are cached with the content hash of their inputs:

```rust
pub struct SemanticCacheEntry {
    pub node_id: NodeId,
    pub input_hash: u64,     // Hash of (source_code + doc_comment + callers + callees)
    pub prompt_template_version: String, // e.g., "entity-summary-v2"
    pub model_id: String,    // Exact model identifier (e.g., "claude-sonnet-4-20250514")
    pub response: Value,
    pub cached_at: DateTime<Utc>,
    pub token_cost: u32,     // For cost tracking
    pub provenance: AnalysisProvenance,
}
```

On incremental update:
1. Recompute input hash for each high-salience node
2. If hash matches cache → skip (source, doc comment, and neighborhood unchanged)
3. If hash differs → re-summarize, update cache

### Cost Tracking

```rust
pub struct LlmCostTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_requests: u64,
    pub estimated_cost_usd: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub doc_comment_skips: u64,  // Entities skipped thanks to good doc comments
}
```

Homer reports LLM costs at the end of each run so users can understand and control spending.

### Reproducibility

LLM outputs are inherently non-deterministic. Homer makes this manageable, not invisible:

**Cache key composition**: The semantic cache key is `(model_id, prompt_template_version, input_hash)` where `input_hash` covers source code, doc comments, and graph neighborhood. Changing the model or editing a prompt template invalidates all affected cache entries, triggering re-summarization on the next run.

**Model version pinning**: The `LlmProviderConfig` requires an exact model identifier (e.g., `claude-sonnet-4-20250514`), not a floating alias (e.g., `claude-sonnet`). When a user updates the model, cache invalidation is automatic — the old cache entries remain but won't match.

**Prompt template versioning**: Each prompt template has a version string (e.g., `"entity-summary-v2"`). Templates are defined in code, not loaded from external files, so they are versioned with the Homer binary. Template version is part of the cache key.

**Temperature**: All summarization calls use `temperature=0` by default (configurable). This minimizes but does not eliminate variation across identical inputs.

### Provenance

Every LLM-derived analysis result carries provenance metadata so consumers can distinguish between evidence-grounded and speculative claims:

```rust
pub enum AnalysisProvenance {
    /// Derived purely from code analysis (deterministic, reproducible)
    Algorithmic { input_node_ids: Vec<NodeId> },
    /// Derived from LLM analysis of code (non-deterministic, cached)
    LlmDerived {
        model_id: String,
        prompt_template: String,
        input_hash: u64,
        /// Evidence nodes that were provided to the LLM as context
        evidence_nodes: Vec<NodeId>,
        /// Confidence: "high" if LLM confirms existing doc comment, "medium" otherwise
        confidence: ProvenanceConfidence,
    },
    /// Derived from combining algorithmic and LLM signals
    Composite { sources: Vec<AnalysisProvenance> },
}

pub enum ProvenanceConfidence {
    /// LLM confirmed/augmented an existing doc comment
    High,
    /// LLM generated from code alone (no prior human description)
    Medium,
    /// LLM output that couldn't be grounded in specific evidence
    Low,
}
```

Renderers use provenance to annotate outputs: high-confidence claims are stated directly; low-confidence claims are prefixed with qualifiers ("Likely...", "Appears to...").

### Evaluation

Homer does not ship with a full evaluation harness, but the specification requires:

1. **Golden repo tests**: At least 3 fixture repos with manually-written expected summaries for high-salience entities. These serve as regression tests — if summary quality degrades after a model or prompt change, the test fails.
2. **Diff-on-rerun**: `homer analyze --diff-summaries` compares current LLM outputs against cached versions and reports the delta. This surfaces silent quality drift without requiring ground truth.
3. **Human-in-the-loop**: The `--review-summaries` flag pauses after each LLM summary and prompts for accept/reject/edit (dev workflow only, not production).
