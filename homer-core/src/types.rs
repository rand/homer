use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};

// ── Typed ID wrappers ──────────────────────────────────────────────

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub i64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(id: i64) -> Self {
                Self(id)
            }
        }
    };
}

typed_id!(NodeId);
typed_id!(HyperedgeId);
typed_id!(AnalysisResultId);
typed_id!(SnapshotId);

// ── Node types ─────────────────────────────────────────────────────

/// Every entity Homer tracks is a node in the hypergraph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    /// A source file tracked by the repository.
    File,
    /// A function or method definition.
    Function,
    /// A type definition (struct, class, enum, interface, etc.).
    Type,
    /// A logical module or directory grouping.
    Module,
    /// A git commit.
    Commit,
    /// A pull request or merge request.
    PullRequest,
    /// An issue or bug report.
    Issue,
    /// A code contributor (author or reviewer).
    Contributor,
    /// A tagged release or version.
    Release,
    /// An abstract concept derived from analysis.
    Concept,
    /// An external package dependency.
    ExternalDep,
    /// A documentation file (README, ADR, guide, etc.).
    Document,
    /// An AI agent prompt or interaction.
    Prompt,
    /// A rule defined for AI agents (e.g., in CLAUDE.md).
    AgentRule,
    /// A recorded AI agent session.
    AgentSession,
}

impl NodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Function => "Function",
            Self::Type => "Type",
            Self::Module => "Module",
            Self::Commit => "Commit",
            Self::PullRequest => "PullRequest",
            Self::Issue => "Issue",
            Self::Contributor => "Contributor",
            Self::Release => "Release",
            Self::Concept => "Concept",
            Self::ExternalDep => "ExternalDep",
            Self::Document => "Document",
            Self::Prompt => "Prompt",
            Self::AgentRule => "AgentRule",
            Self::AgentSession => "AgentSession",
        }
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A node in the Homer hypergraph — the fundamental unit of repository data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Canonical name: file path, function qualified name, commit SHA, etc.
    pub name: String,
    /// Content hash for change detection.
    pub content_hash: Option<u64>,
    /// When this node was last extracted/updated.
    pub last_extracted: DateTime<Utc>,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

// ── Hyperedge types ────────────────────────────────────────────────

/// A hyperedge connects one or more nodes with a typed relationship.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HyperedgeKind {
    /// Commit → File: a commit modifies one or more files.
    Modifies,
    /// File → File: one file imports symbols from another.
    Imports,
    /// Function → Function: one function calls another.
    Calls,
    /// Type → Type: one type inherits from or implements another.
    Inherits,
    /// Symbol reference → definition resolution.
    Resolves,
    /// Contributor → Commit: authorship attribution.
    Authored,
    /// Contributor → `PullRequest`: code review participation.
    Reviewed,
    /// Commit → Commit: a merge commit includes other commits.
    Includes,
    /// File → Module: file membership in a directory/module.
    BelongsTo,
    /// Module → `ExternalDep`: a module depends on an external package.
    DependsOn,
    /// Name → Name: two names refer to the same entity (re-exports).
    Aliases,
    /// Document → Entity: a document describes an entity.
    Documents,
    /// `AgentSession` → File: files referenced in an agent prompt.
    PromptReferences,
    /// `AgentSession` → File: files modified during an agent session.
    PromptModifiedFiles,
    /// `AgentSession` → `AgentSession`: sessions covering related work.
    RelatedPrompts,
    /// File → File: files that frequently change together (analysis-derived).
    CoChanges,
    /// Node → Community: membership in a detected community cluster.
    ClusterMembers,
    /// Community → Node: a community encompasses a set of nodes.
    Encompasses,
}

impl HyperedgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Modifies => "Modifies",
            Self::Imports => "Imports",
            Self::Calls => "Calls",
            Self::Inherits => "Inherits",
            Self::Resolves => "Resolves",
            Self::Authored => "Authored",
            Self::Reviewed => "Reviewed",
            Self::Includes => "Includes",
            Self::BelongsTo => "BelongsTo",
            Self::DependsOn => "DependsOn",
            Self::Aliases => "Aliases",
            Self::Documents => "Documents",
            Self::PromptReferences => "PromptReferences",
            Self::PromptModifiedFiles => "PromptModifiedFiles",
            Self::RelatedPrompts => "RelatedPrompts",
            Self::CoChanges => "CoChanges",
            Self::ClusterMembers => "ClusterMembers",
            Self::Encompasses => "Encompasses",
        }
    }
}

impl std::fmt::Display for HyperedgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hyperedge {
    pub id: HyperedgeId,
    pub kind: HyperedgeKind,
    /// The nodes participating in this relationship.
    pub members: Vec<HyperedgeMember>,
    /// Edge-level confidence score (0.0 - 1.0).
    pub confidence: f64,
    /// When this edge was created/last updated.
    pub last_updated: DateTime<Utc>,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A node's participation in a hyperedge, including its role and position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperedgeMember {
    /// The node participating in this relationship.
    pub node_id: NodeId,
    /// Role within this edge (e.g., "caller"/"callee", "author"/"commit").
    pub role: String,
    /// Ordering within the edge.
    pub position: u32,
}

// ── Analysis types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnalysisKind {
    /// How often a file changes (commit count, percentile).
    ChangeFrequency,
    /// Rate of code churn (lines added/deleted over time).
    ChurnVelocity,
    /// Bus factor and contributor distribution for a file.
    ContributorConcentration,
    /// Documentation coverage percentage for entities.
    DocumentationCoverage,
    /// How recently documentation was updated relative to code.
    DocumentationFreshness,
    /// Files most frequently referenced in agent prompts.
    PromptHotspot,
    /// Files where agents most frequently self-correct.
    CorrectionHotspot,
    /// `PageRank` centrality score from the call/import graph.
    PageRank,
    /// Betweenness centrality — how often a node bridges paths.
    BetweennessCentrality,
    /// HITS authority and hub scores.
    HITSScore,
    /// Weighted composite of all centrality and behavioral signals.
    CompositeSalience,
    /// Which community cluster a node belongs to (Louvain).
    CommunityAssignment,
    /// How a node's centrality is changing across snapshots.
    CentralityTrend,
    /// Whether community boundaries are shifting over time.
    ArchitecturalDrift,
    /// Stability classification combining churn and centrality.
    StabilityClassification,
    /// Dominant naming convention (`snake_case`, `camelCase`, etc.).
    NamingPattern,
    /// Testing framework and patterns detected in the repo.
    TestingPattern,
    /// Error handling patterns (Result, exceptions, error codes).
    ErrorHandlingPattern,
    /// Documentation style and coverage conventions.
    DocumentationStylePattern,
    /// Validation of agent rule files against actual codebase behavior.
    AgentRuleValidation,
    /// Common task patterns derived from agent session analysis.
    TaskPattern,
    /// Domain-specific vocabulary extracted from code and prompts.
    DomainVocabulary,
    /// LLM-generated natural language summary of an entity.
    SemanticSummary,
    /// LLM-generated design rationale for an architectural choice.
    DesignRationale,
    /// LLM-generated invariant description for a type or function.
    InvariantDescription,
}

impl AnalysisKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChangeFrequency => "ChangeFrequency",
            Self::ChurnVelocity => "ChurnVelocity",
            Self::ContributorConcentration => "ContributorConcentration",
            Self::DocumentationCoverage => "DocumentationCoverage",
            Self::DocumentationFreshness => "DocumentationFreshness",
            Self::PromptHotspot => "PromptHotspot",
            Self::CorrectionHotspot => "CorrectionHotspot",
            Self::PageRank => "PageRank",
            Self::BetweennessCentrality => "BetweennessCentrality",
            Self::HITSScore => "HITSScore",
            Self::CompositeSalience => "CompositeSalience",
            Self::CommunityAssignment => "CommunityAssignment",
            Self::CentralityTrend => "CentralityTrend",
            Self::ArchitecturalDrift => "ArchitecturalDrift",
            Self::StabilityClassification => "StabilityClassification",
            Self::NamingPattern => "NamingPattern",
            Self::TestingPattern => "TestingPattern",
            Self::ErrorHandlingPattern => "ErrorHandlingPattern",
            Self::DocumentationStylePattern => "DocumentationStylePattern",
            Self::AgentRuleValidation => "AgentRuleValidation",
            Self::TaskPattern => "TaskPattern",
            Self::DomainVocabulary => "DomainVocabulary",
            Self::SemanticSummary => "SemanticSummary",
            Self::DesignRationale => "DesignRationale",
            Self::InvariantDescription => "InvariantDescription",
        }
    }
}

/// A stored analysis result — the output of an analyzer for a specific node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Unique identifier for this result.
    pub id: AnalysisResultId,
    /// The node this analysis was computed for.
    pub node_id: NodeId,
    /// Which analyzer produced this result.
    pub kind: AnalysisKind,
    /// Structured result data (JSON).
    pub data: serde_json::Value,
    /// Hash of the inputs that produced this result (for invalidation).
    pub input_hash: u64,
    pub computed_at: DateTime<Utc>,
}

// ── Salience & stability classification ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SalienceClass {
    /// High centrality, high churn — active hotspot
    ActiveHotspot,
    /// High centrality, low churn — stable foundation
    FoundationalStable,
    /// Low centrality, high churn — peripheral activity
    PeripheralActive,
    /// Low centrality, low churn — stable leaf
    QuietLeaf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StabilityClass {
    /// Rarely changes, high centrality — stable core
    StableCore,
    /// Frequently changes, high centrality — active development
    ActiveCore,
    /// Rarely changes, low centrality — stable leaf
    StableLeaf,
    /// Frequently changes, low centrality — active leaf
    ActiveLeaf,
}

// ── Batch result ───────────────────────────────────────────────────

/// Result of processing a batch of items — individual failures don't abort the pipeline.
#[derive(Debug)]
pub struct BatchResult<T> {
    pub successes: Vec<T>,
    pub failures: Vec<(PathBuf, crate::error::HomerError)>,
}

impl<T> BatchResult<T> {
    pub fn new() -> Self {
        Self {
            successes: Vec::new(),
            failures: Vec::new(),
        }
    }

    pub fn success_count(&self) -> usize {
        self.successes.len()
    }

    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }
}

impl<T> Default for BatchResult<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Store query types ──────────────────────────────────────────────

/// Filter for finding nodes in the store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeFilter {
    /// Only return nodes of this kind.
    pub kind: Option<NodeKind>,
    /// Only return nodes whose name starts with this prefix.
    pub name_prefix: Option<String>,
    /// Only return nodes whose name contains this substring.
    pub name_contains: Option<String>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

/// Scope for full-text search queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchScope {
    /// Only search content of these types (e.g., "code", "doc").
    pub content_types: Option<Vec<String>>,
    /// Only search nodes of these kinds.
    pub node_kinds: Option<Vec<NodeKind>>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

/// A single full-text search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// The node that matched the search query.
    pub node_id: NodeId,
    /// What type of content matched (e.g., "code", "doc").
    pub content_type: String,
    /// Excerpt of the matching content.
    pub snippet: String,
    /// Relevance score (higher is more relevant).
    pub rank: f64,
}

/// Summary statistics for the store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreStats {
    /// Total number of nodes in the hypergraph.
    pub total_nodes: u64,
    /// Total number of hyperedges.
    pub total_edges: u64,
    /// Total number of stored analysis results.
    pub total_analyses: u64,
    /// Node count broken down by `NodeKind`.
    pub nodes_by_kind: HashMap<String, u64>,
    /// Edge count broken down by `HyperedgeKind`.
    pub edges_by_kind: HashMap<String, u64>,
    /// Database file size in bytes.
    pub db_size_bytes: u64,
}

/// Subgraph filter for loading graphs into memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubgraphFilter {
    /// Load the full graph.
    Full,
    /// N-hop neighborhood of specific nodes.
    Neighborhood { centers: Vec<NodeId>, hops: u32 },
    /// Only nodes with composite salience above threshold.
    HighSalience { min_score: f64 },
    /// Only nodes within a directory prefix.
    Module { path_prefix: String },
    /// Only nodes of specific kinds.
    OfKind { kinds: Vec<NodeKind> },
    /// Intersection of multiple filters.
    And(Vec<SubgraphFilter>),
}

/// A petgraph `DiGraph` loaded from the store, with `NodeId` ↔ `NodeIndex` mapping.
#[derive(Debug)]
pub struct InMemoryGraph {
    pub graph: DiGraph<NodeId, f64>,
    pub node_to_index: HashMap<NodeId, NodeIndex>,
    pub index_to_node: HashMap<NodeIndex, NodeId>,
}

impl InMemoryGraph {
    /// Build a graph from hyperedges, extracting directed pairs by role or position.
    pub fn from_edges(edges: &[Hyperedge]) -> Self {
        let estimated_nodes = edges.len();
        let mut graph = DiGraph::<NodeId, f64>::with_capacity(estimated_nodes, edges.len());
        let mut node_to_index: HashMap<NodeId, NodeIndex> = HashMap::with_capacity(estimated_nodes);
        let mut index_to_node: HashMap<NodeIndex, NodeId> = HashMap::with_capacity(estimated_nodes);

        for edge in edges {
            for member in &edge.members {
                node_to_index.entry(member.node_id).or_insert_with(|| {
                    let idx = graph.add_node(member.node_id);
                    index_to_node.insert(idx, member.node_id);
                    idx
                });
            }
        }

        for edge in edges {
            let (source, target) = extract_directed_pair(&edge.members);
            if let (Some(&src_idx), Some(&tgt_idx)) =
                (node_to_index.get(&source), node_to_index.get(&target))
            {
                graph.add_edge(src_idx, tgt_idx, edge.confidence);
            }
        }

        Self {
            graph,
            node_to_index,
            index_to_node,
        }
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

/// Extract a directed (source, target) pair from hyperedge members.
/// Uses roles ("caller"/"callee", "source"/"target") or falls back to position ordering.
pub fn extract_directed_pair(members: &[HyperedgeMember]) -> (NodeId, NodeId) {
    if members.len() < 2 {
        let id = members.first().map_or(NodeId(0), |m| m.node_id);
        return (id, id);
    }

    let source_roles = ["caller", "source", "importer"];
    let target_roles = ["callee", "target", "imported"];

    let source = members
        .iter()
        .find(|m| source_roles.contains(&m.role.as_str()));
    let target = members
        .iter()
        .find(|m| target_roles.contains(&m.role.as_str()));

    if let (Some(s), Some(t)) = (source, target) {
        return (s.node_id, t.node_id);
    }

    let mut sorted = members.to_vec();
    sorted.sort_by_key(|m| m.position);
    (sorted[0].node_id, sorted[1].node_id)
}

/// Metadata about a stored snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Unique identifier for this snapshot.
    pub id: SnapshotId,
    /// Human-readable label (e.g., "v1.0.0" or "auto-50").
    pub label: String,
    /// When this snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// Total number of nodes at snapshot time.
    pub node_count: u64,
    /// Total number of edges at snapshot time.
    pub edge_count: u64,
}

/// Graph diff between two snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphDiff {
    /// Nodes present in the newer snapshot but not the older.
    pub added_nodes: Vec<NodeId>,
    /// Nodes present in the older snapshot but not the newer.
    pub removed_nodes: Vec<NodeId>,
    /// Edges present in the newer snapshot but not the older.
    pub added_edges: Vec<HyperedgeId>,
    /// Edges present in the older snapshot but not the newer.
    pub removed_edges: Vec<HyperedgeId>,
}

// ── Extractor-specific types ───────────────────────────────────────

/// Per-file diff statistics from the git extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiffStats {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub status: DiffStatus,
    pub lines_added: u32,
    pub lines_deleted: u32,
    /// Per-hunk diff metadata for fine-grained analysis.
    pub hunks: Vec<DiffHunk>,
}

/// A contiguous region of changes within a file diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Starting line number in the old file (1-based).
    pub old_start: u32,
    /// Number of lines from the old file in this hunk.
    pub old_lines: u32,
    /// Starting line number in the new file (1-based).
    pub new_start: u32,
    /// Number of lines from the new file in this hunk.
    pub new_lines: u32,
}

/// Status of a file in a git diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffStatus {
    /// File was newly added.
    Added,
    /// File content was modified.
    Modified,
    /// File was deleted.
    Deleted,
    /// File was renamed (possibly with modifications).
    Renamed,
    /// File was copied.
    Copied,
}

/// Document type classification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentType {
    /// README file (project overview).
    Readme,
    /// Contributing guidelines.
    Contributing,
    /// Architecture documentation.
    Architecture,
    /// Architecture Decision Record.
    Adr,
    /// Release changelog or history.
    Changelog,
    /// API reference documentation.
    ApiDoc,
    /// How-to guide or tutorial.
    Guide,
    /// Operational runbook or playbook.
    Runbook,
    /// Unclassified documentation.
    Other,
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_serde_round_trip() {
        for kind in [
            NodeKind::File,
            NodeKind::Function,
            NodeKind::Type,
            NodeKind::Module,
            NodeKind::Commit,
            NodeKind::PullRequest,
            NodeKind::Issue,
            NodeKind::Contributor,
            NodeKind::Release,
            NodeKind::Concept,
            NodeKind::ExternalDep,
            NodeKind::Document,
            NodeKind::Prompt,
            NodeKind::AgentRule,
            NodeKind::AgentSession,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: NodeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn hyperedge_kind_serde_round_trip() {
        for kind in [
            HyperedgeKind::Modifies,
            HyperedgeKind::Imports,
            HyperedgeKind::Calls,
            HyperedgeKind::Inherits,
            HyperedgeKind::Resolves,
            HyperedgeKind::Authored,
            HyperedgeKind::Reviewed,
            HyperedgeKind::Includes,
            HyperedgeKind::BelongsTo,
            HyperedgeKind::DependsOn,
            HyperedgeKind::Aliases,
            HyperedgeKind::Documents,
            HyperedgeKind::PromptReferences,
            HyperedgeKind::PromptModifiedFiles,
            HyperedgeKind::RelatedPrompts,
            HyperedgeKind::CoChanges,
            HyperedgeKind::ClusterMembers,
            HyperedgeKind::Encompasses,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: HyperedgeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn analysis_kind_serde_round_trip() {
        let kind = AnalysisKind::CompositeSalience;
        let json = serde_json::to_string(&kind).unwrap();
        let back: AnalysisKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn node_serde_round_trip() {
        let node = Node {
            id: NodeId(42),
            kind: NodeKind::Function,
            name: "src::auth::validate_token".to_string(),
            content_hash: Some(0xDEAD_BEEF),
            last_extracted: Utc::now(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("lines".to_string(), serde_json::json!(150));
                m
            },
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, node.id);
        assert_eq!(back.kind, node.kind);
        assert_eq!(back.name, node.name);
        assert_eq!(back.content_hash, node.content_hash);
    }

    #[test]
    fn hyperedge_serde_round_trip() {
        let edge = Hyperedge {
            id: HyperedgeId(1),
            kind: HyperedgeKind::Calls,
            members: vec![
                HyperedgeMember {
                    node_id: NodeId(10),
                    role: "caller".to_string(),
                    position: 0,
                },
                HyperedgeMember {
                    node_id: NodeId(20),
                    role: "callee".to_string(),
                    position: 1,
                },
            ],
            confidence: 0.95,
            last_updated: Utc::now(),
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: Hyperedge = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, edge.id);
        assert_eq!(back.kind, edge.kind);
        assert_eq!(back.members.len(), 2);
    }

    #[test]
    fn typed_id_display() {
        assert_eq!(NodeId(42).to_string(), "42");
        assert_eq!(HyperedgeId(7).to_string(), "7");
    }

    #[test]
    fn batch_result_tracking() {
        let mut batch = BatchResult::<String>::new();
        assert_eq!(batch.success_count(), 0);
        assert!(!batch.has_failures());

        batch.successes.push("ok".to_string());
        assert_eq!(batch.success_count(), 1);
        assert_eq!(batch.failure_count(), 0);
    }

    #[test]
    fn salience_and_stability_serde() {
        let s = SalienceClass::FoundationalStable;
        let json = serde_json::to_string(&s).unwrap();
        let back: SalienceClass = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);

        let st = StabilityClass::StableCore;
        let json = serde_json::to_string(&st).unwrap();
        let back: StabilityClass = serde_json::from_str(&json).unwrap();
        assert_eq!(st, back);
    }

    #[test]
    fn diff_status_serde() {
        let s = DiffStatus::Renamed;
        let json = serde_json::to_string(&s).unwrap();
        let back: DiffStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    // ── Property-based serde round-trip tests ─────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_node_kind() -> impl Strategy<Value = NodeKind> {
            prop_oneof![
                Just(NodeKind::File),
                Just(NodeKind::Function),
                Just(NodeKind::Type),
                Just(NodeKind::Module),
                Just(NodeKind::Commit),
                Just(NodeKind::PullRequest),
                Just(NodeKind::Issue),
                Just(NodeKind::Contributor),
                Just(NodeKind::Release),
                Just(NodeKind::Concept),
                Just(NodeKind::ExternalDep),
                Just(NodeKind::Document),
                Just(NodeKind::Prompt),
                Just(NodeKind::AgentRule),
                Just(NodeKind::AgentSession),
            ]
        }

        fn arb_edge_kind() -> impl Strategy<Value = HyperedgeKind> {
            prop_oneof![
                Just(HyperedgeKind::Modifies),
                Just(HyperedgeKind::Imports),
                Just(HyperedgeKind::Calls),
                Just(HyperedgeKind::Inherits),
                Just(HyperedgeKind::Resolves),
                Just(HyperedgeKind::Authored),
                Just(HyperedgeKind::Reviewed),
                Just(HyperedgeKind::Includes),
                Just(HyperedgeKind::BelongsTo),
                Just(HyperedgeKind::DependsOn),
                Just(HyperedgeKind::Aliases),
                Just(HyperedgeKind::Documents),
                Just(HyperedgeKind::PromptReferences),
                Just(HyperedgeKind::PromptModifiedFiles),
                Just(HyperedgeKind::RelatedPrompts),
                Just(HyperedgeKind::CoChanges),
                Just(HyperedgeKind::ClusterMembers),
                Just(HyperedgeKind::Encompasses),
            ]
        }

        fn arb_analysis_kind() -> impl Strategy<Value = AnalysisKind> {
            prop_oneof![
                Just(AnalysisKind::ChangeFrequency),
                Just(AnalysisKind::ChurnVelocity),
                Just(AnalysisKind::ContributorConcentration),
                Just(AnalysisKind::DocumentationCoverage),
                Just(AnalysisKind::PageRank),
                Just(AnalysisKind::BetweennessCentrality),
                Just(AnalysisKind::HITSScore),
                Just(AnalysisKind::CompositeSalience),
                Just(AnalysisKind::CommunityAssignment),
                Just(AnalysisKind::NamingPattern),
                Just(AnalysisKind::TaskPattern),
                Just(AnalysisKind::SemanticSummary),
            ]
        }

        fn arb_salience() -> impl Strategy<Value = SalienceClass> {
            prop_oneof![
                Just(SalienceClass::ActiveHotspot),
                Just(SalienceClass::FoundationalStable),
                Just(SalienceClass::PeripheralActive),
                Just(SalienceClass::QuietLeaf),
            ]
        }

        fn arb_stability() -> impl Strategy<Value = StabilityClass> {
            prop_oneof![
                Just(StabilityClass::StableCore),
                Just(StabilityClass::ActiveCore),
                Just(StabilityClass::StableLeaf),
                Just(StabilityClass::ActiveLeaf),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            #[test]
            fn node_kind_serde_roundtrip(kind in arb_node_kind()) {
                let json = serde_json::to_string(&kind).unwrap();
                let back: NodeKind = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, kind);
            }

            #[test]
            fn edge_kind_serde_roundtrip(kind in arb_edge_kind()) {
                let json = serde_json::to_string(&kind).unwrap();
                let back: HyperedgeKind = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, kind);
            }

            #[test]
            fn analysis_kind_serde_roundtrip(kind in arb_analysis_kind()) {
                let json = serde_json::to_string(&kind).unwrap();
                let back: AnalysisKind = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, kind);
            }

            #[test]
            fn salience_serde_roundtrip(s in arb_salience()) {
                let json = serde_json::to_string(&s).unwrap();
                let back: SalienceClass = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, s);
            }

            #[test]
            fn stability_serde_roundtrip(s in arb_stability()) {
                let json = serde_json::to_string(&s).unwrap();
                let back: StabilityClass = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, s);
            }

            #[test]
            fn node_kind_as_str_stable(kind in arb_node_kind()) {
                let s = kind.as_str();
                prop_assert!(!s.is_empty());
                prop_assert_eq!(kind.to_string(), s);
            }

            #[test]
            fn edge_kind_as_str_stable(kind in arb_edge_kind()) {
                let s = kind.as_str();
                prop_assert!(!s.is_empty());
                prop_assert_eq!(kind.to_string(), s);
            }

            #[test]
            fn typed_id_roundtrip(id in any::<i64>()) {
                let node_id = NodeId(id);
                let json = serde_json::to_string(&node_id).unwrap();
                let back: NodeId = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, node_id);
            }
        }
    }
}
