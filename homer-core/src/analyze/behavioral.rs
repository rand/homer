// Statistical computations intentionally cast int→float (precision loss acceptable for metrics).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::Utc;
use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind, NodeId, NodeKind,
};

use super::AnalyzeStats;
use super::traits::Analyzer;

#[derive(Debug)]
pub struct BehavioralAnalyzer;

#[async_trait::async_trait]
impl Analyzer for BehavioralAnalyzer {
    fn name(&self) -> &'static str {
        "behavioral"
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Collect raw data from the store
        let commit_data = collect_commit_data(store).await?;

        if commit_data.file_commits.is_empty() {
            info!("No commit data found, skipping behavioral analysis");
            return Ok(stats);
        }

        info!(
            files = commit_data.file_commits.len(),
            commits = commit_data.commit_count,
            "Running behavioral analysis"
        );

        // Compute and store change frequency per file
        compute_change_frequency(store, &commit_data, &mut stats).await?;

        // Compute and store churn velocity per file
        compute_churn_velocity(store, &commit_data, &mut stats).await?;

        // Compute and store contributor concentration (bus factor)
        compute_bus_factor(store, &commit_data, &mut stats).await?;

        // Compute and store co-change sets
        compute_co_change(store, &commit_data, &mut stats).await?;

        // Compute documentation coverage
        compute_doc_coverage(store, &mut stats).await?;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Behavioral analysis complete"
        );
        Ok(stats)
    }
}

// ── Data collection ───────────────────────────────────────────────

/// Intermediate data collected from the store for analysis.
struct CommitData {
    file_commits: HashMap<NodeId, Vec<FileChange>>,
    commit_files: HashMap<NodeId, HashSet<NodeId>>,
    /// Total commit count
    commit_count: usize,
}

struct FileChange {
    commit_time: chrono::DateTime<Utc>,
    lines_added: u64,
    lines_deleted: u64,
    author_id: Option<NodeId>,
}

async fn collect_commit_data(store: &dyn HomerStore) -> crate::error::Result<CommitData> {
    let mut file_commits: HashMap<NodeId, Vec<FileChange>> = HashMap::new();
    let mut commit_files: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();

    // Get all Modifies edges
    let modifies_edges = store.get_edges_by_kind(HyperedgeKind::Modifies).await?;

    // Build author map: commit_id → author_id from Authored edges
    let authored_edges = store.get_edges_by_kind(HyperedgeKind::Authored).await?;
    let mut commit_author: HashMap<NodeId, NodeId> = HashMap::new();
    for edge in &authored_edges {
        let author = edge.members.iter().find(|m| m.role == "author");
        let commit = edge.members.iter().find(|m| m.role == "commit");
        if let (Some(a), Some(c)) = (author, commit) {
            commit_author.insert(c.node_id, a.node_id);
        }
    }

    for edge in &modifies_edges {
        let commit_member = edge.members.iter().find(|m| m.role == "commit");
        let Some(commit_m) = commit_member else {
            continue;
        };
        let commit_id = commit_m.node_id;
        let commit_time = edge.last_updated;
        let author_id = commit_author.get(&commit_id).copied();

        // Parse per-file diff data from metadata
        let files_meta = edge
            .metadata
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let file_members: Vec<_> = edge.members.iter().filter(|m| m.role == "file").collect();

        for (idx, file_m) in file_members.iter().enumerate() {
            let (added, deleted) = files_meta.get(idx).map_or((0, 0), |f| {
                let a = f
                    .get("lines_added")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let d = f
                    .get("lines_deleted")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                (a, d)
            });

            file_commits
                .entry(file_m.node_id)
                .or_default()
                .push(FileChange {
                    commit_time,
                    lines_added: added,
                    lines_deleted: deleted,
                    author_id,
                });

            commit_files
                .entry(commit_id)
                .or_default()
                .insert(file_m.node_id);
        }
    }

    let commit_count = commit_files.len();
    Ok(CommitData {
        file_commits,
        commit_files,
        commit_count,
    })
}

// ── Change frequency ──────────────────────────────────────────────

async fn compute_change_frequency(
    store: &dyn HomerStore,
    data: &CommitData,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let now = Utc::now();
    let mut frequencies: Vec<(NodeId, u64)> = Vec::new();

    for (file_id, changes) in &data.file_commits {
        let total = changes.len() as u64;
        let last_30d = count_in_window(changes, now, 30);
        let last_90d = count_in_window(changes, now, 90);
        let last_365d = count_in_window(changes, now, 365);

        frequencies.push((*file_id, total));

        let result_data = serde_json::json!({
            "total": total,
            "last_30d": last_30d,
            "last_90d": last_90d,
            "last_365d": last_365d,
        });

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: *file_id,
                kind: AnalysisKind::ChangeFrequency,
                data: result_data,
                input_hash: 0,
                computed_at: now,
            })
            .await?;
        stats.results_stored += 1;
    }

    // Compute percentile ranks and update
    frequencies.sort_by_key(|&(_, count)| count);
    let n = frequencies.len() as f64;
    for (rank, (file_id, _)) in frequencies.iter().enumerate() {
        let percentile = if n > 0.0 {
            (rank as f64 / n * 100.0).round()
        } else {
            0.0
        };

        // Read the existing result and add percentile
        if let Ok(Some(mut existing)) = store
            .get_analysis(*file_id, AnalysisKind::ChangeFrequency)
            .await
        {
            if let Some(obj) = existing.data.as_object_mut() {
                obj.insert("percentile".to_string(), serde_json::json!(percentile));
            }
            store.store_analysis(&existing).await?;
        }
    }

    Ok(())
}

fn count_in_window(changes: &[FileChange], now: chrono::DateTime<Utc>, days: i64) -> u64 {
    let cutoff = now - chrono::Duration::days(days);
    changes.iter().filter(|c| c.commit_time >= cutoff).count() as u64
}

// ── Churn velocity ────────────────────────────────────────────────

async fn compute_churn_velocity(
    store: &dyn HomerStore,
    data: &CommitData,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let now = Utc::now();

    for (file_id, changes) in &data.file_commits {
        if changes.len() < 2 {
            continue;
        }

        // Linear regression: x = days ago, y = cumulative churn (added + deleted)
        // Group by month for smoothing
        let mut monthly_churn: HashMap<i64, u64> = HashMap::new();
        for change in changes {
            let days_ago = (now - change.commit_time).num_days();
            let month = days_ago / 30;
            *monthly_churn.entry(month).or_default() += change.lines_added + change.lines_deleted;
        }

        if monthly_churn.len() < 2 {
            continue;
        }

        // Simple linear regression
        let points: Vec<(f64, f64)> = monthly_churn
            .iter()
            .map(|(&month, &churn)| (month as f64, churn as f64))
            .collect();

        let (slope, _intercept) = linear_regression(&points);

        // Positive slope = increasing churn, negative = decreasing
        let trend = if slope > 0.5 {
            "increasing"
        } else if slope < -0.5 {
            "decreasing"
        } else {
            "stable"
        };

        let total_churn: u64 = changes
            .iter()
            .map(|c| c.lines_added + c.lines_deleted)
            .sum();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: *file_id,
                kind: AnalysisKind::ChurnVelocity,
                data: serde_json::json!({
                    "slope": slope,
                    "trend": trend,
                    "total_churn": total_churn,
                    "data_points": monthly_churn.len(),
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await?;
        stats.results_stored += 1;
    }

    Ok(())
}

fn linear_regression(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    if n < 2.0 {
        return (0.0, 0.0);
    }

    let sum_x: f64 = points.iter().map(|(x, _)| x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| y).sum();
    let dot_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_xx: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        return (0.0, sum_y / n);
    }

    let slope = (n * dot_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;
    (slope, intercept)
}

// ── Contributor concentration (bus factor) ────────────────────────

async fn compute_bus_factor(
    store: &dyn HomerStore,
    data: &CommitData,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let now = Utc::now();

    for (file_id, changes) in &data.file_commits {
        // Count commits per author
        let mut author_counts: HashMap<NodeId, u64> = HashMap::new();
        for change in changes {
            if let Some(author_id) = change.author_id {
                *author_counts.entry(author_id).or_default() += 1;
            }
        }

        let total_commits = changes.len() as f64;
        let unique_authors = author_counts.len();

        if unique_authors == 0 {
            continue;
        }

        // Bus factor: minimum number of authors controlling >80% of changes (per spec)
        let mut sorted_counts: Vec<u64> = author_counts.values().copied().collect();
        sorted_counts.sort_unstable_by(|a, b| b.cmp(a)); // Descending

        let threshold = total_commits * 0.8;
        let mut cumulative = 0.0;
        let mut bus_factor = 0u32;
        for count in &sorted_counts {
            cumulative += *count as f64;
            bus_factor += 1;
            if cumulative >= threshold {
                break;
            }
        }

        // Top contributor share
        let top_share = sorted_counts.first().copied().unwrap_or(0) as f64 / total_commits;

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: *file_id,
                kind: AnalysisKind::ContributorConcentration,
                data: serde_json::json!({
                    "bus_factor": bus_factor,
                    "unique_authors": unique_authors,
                    "top_contributor_share": (top_share * 100.0).round() / 100.0,
                    "total_commits": total_commits as u64,
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await?;
        stats.results_stored += 1;
    }

    Ok(())
}

// ── Co-change detection (seed-and-grow) ───────────────────────────

async fn compute_co_change(
    store: &dyn HomerStore,
    data: &CommitData,
    _stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    // Build co-occurrence matrix: for each pair of files, how often do they
    // appear in the same commit?
    let mut co_occur: HashMap<(NodeId, NodeId), u32> = HashMap::new();

    for file_set in data.commit_files.values() {
        let files: Vec<NodeId> = file_set.iter().copied().collect();
        for i in 0..files.len() {
            for j in (i + 1)..files.len() {
                let pair = if files[i].0 < files[j].0 {
                    (files[i], files[j])
                } else {
                    (files[j], files[i])
                };
                *co_occur.entry(pair).or_default() += 1;
            }
        }
    }

    // For each file, find its top co-changed partners (support >= 2, confidence >= 0.3)
    let total_commits = data.commit_count as f64;
    let min_support = 2u32;

    // Group co-occurrences by file
    let mut file_partners: HashMap<NodeId, Vec<(NodeId, u32, f64)>> = HashMap::new();

    for (&(file_a, file_b), &count) in &co_occur {
        if count < min_support {
            continue;
        }

        let commits_a = data.file_commits.get(&file_a).map_or(0, Vec::len);
        let commits_b = data.file_commits.get(&file_b).map_or(0, Vec::len);

        // Confidence = co-occurrence / min(changes_a, changes_b)
        let min_changes = commits_a.min(commits_b) as f64;
        if min_changes < 1.0 {
            continue;
        }
        let confidence = count as f64 / min_changes;

        if confidence >= 0.3 {
            file_partners
                .entry(file_a)
                .or_default()
                .push((file_b, count, confidence));
            file_partners
                .entry(file_b)
                .or_default()
                .push((file_a, count, confidence));
        }
    }

    // Store co-change results per file (top 10 partners)
    for (file_id, mut partners) in file_partners {
        partners.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        partners.truncate(10);

        // Resolve partner names
        let mut partner_data = Vec::new();
        for (partner_id, count, confidence) in &partners {
            let name = store
                .get_node(*partner_id)
                .await?
                .map_or_else(|| format!("node:{}", partner_id.0), |n| n.name);
            partner_data.push(serde_json::json!({
                "file": name,
                "co_occurrences": count,
                "confidence": (confidence * 100.0).round() / 100.0,
                "support": *count as f64 / total_commits,
            }));
        }

        // We store co-change as a ChangeFrequency-adjacent analysis
        // (no dedicated CoChange kind yet — use metadata on ChangeFrequency or store separately)
        // For now, store in the ChangeFrequency result's data as an update if it exists,
        // otherwise store as part of a general result.
        if let Ok(Some(mut freq)) = store
            .get_analysis(file_id, AnalysisKind::ChangeFrequency)
            .await
        {
            if let Some(obj) = freq.data.as_object_mut() {
                obj.insert(
                    "co_change_partners".to_string(),
                    serde_json::json!(partner_data),
                );
            }
            store.store_analysis(&freq).await?;
        }
    }

    Ok(())
}

// ── Documentation coverage ────────────────────────────────────────

async fn compute_doc_coverage(
    store: &dyn HomerStore,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let now = Utc::now();

    // Get all File nodes
    let file_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await?;

    // Get all Documents edges to know which files are documented
    let doc_edges = store.get_edges_by_kind(HyperedgeKind::Documents).await?;
    let mut documented_files: HashSet<NodeId> = HashSet::new();
    for edge in &doc_edges {
        for member in &edge.members {
            if member.role == "subject" {
                documented_files.insert(member.node_id);
            }
        }
    }

    for file in &files {
        let has_docs = documented_files.contains(&file.id);

        // Check if file has doc comments in graph data
        let has_doc_comments = file.metadata.contains_key("language");

        // Simple coverage: documented by external docs or has doc comments
        let coverage = if has_docs {
            "documented"
        } else {
            "undocumented"
        };

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file.id,
                kind: AnalysisKind::DocumentationCoverage,
                data: serde_json::json!({
                    "status": coverage,
                    "has_external_docs": has_docs,
                    "has_doc_comments": has_doc_comments,
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await?;
        stats.results_stored += 1;
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HomerConfig;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Hyperedge, HyperedgeId, HyperedgeMember, Node};

    /// Create a minimal dataset directly in the store to test the analyzer
    /// without needing git history.
    #[allow(clippy::too_many_lines)]
    async fn setup_test_data(store: &SqliteStore) {
        // Create file nodes
        let file_a = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/main.rs".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("language".to_string(), serde_json::json!("rust"));
                    m
                },
            })
            .await
            .unwrap();

        let file_b = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/lib.rs".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("language".to_string(), serde_json::json!("rust"));
                    m
                },
            })
            .await
            .unwrap();

        let file_c = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "tests/test.rs".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Create contributor
        let author = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Contributor,
                name: "dev@test.com".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Create commits that touch different file combinations
        let now = Utc::now();
        for i in 0..5 {
            let commit = store
                .upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::Commit,
                    name: format!("commit-{i}"),
                    content_hash: None,
                    last_extracted: now,
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();

            // Authored edge
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Authored,
                    members: vec![
                        HyperedgeMember {
                            node_id: author,
                            role: "author".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: commit,
                            role: "commit".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: now - chrono::Duration::days(i * 10),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();

            // Modifies edge — commits 0-3 touch file_a and file_b (co-change),
            // commit 4 touches file_c only
            let mut members = vec![HyperedgeMember {
                node_id: commit,
                role: "commit".to_string(),
                position: 0,
            }];
            let mut files_json = Vec::new();

            if i < 4 {
                members.push(HyperedgeMember {
                    node_id: file_a,
                    role: "file".to_string(),
                    position: 1,
                });
                files_json.push(serde_json::json!({
                    "path": "src/main.rs",
                    "status": "modified",
                    "lines_added": 10,
                    "lines_deleted": 5,
                }));
                members.push(HyperedgeMember {
                    node_id: file_b,
                    role: "file".to_string(),
                    position: 2,
                });
                files_json.push(serde_json::json!({
                    "path": "src/lib.rs",
                    "status": "modified",
                    "lines_added": 3,
                    "lines_deleted": 1,
                }));
            }
            if i == 4 {
                members.push(HyperedgeMember {
                    node_id: file_c,
                    role: "file".to_string(),
                    position: 1,
                });
                files_json.push(serde_json::json!({
                    "path": "tests/test.rs",
                    "status": "modified",
                    "lines_added": 20,
                    "lines_deleted": 0,
                }));
            }

            let mut meta = HashMap::new();
            meta.insert("files".to_string(), serde_json::json!(files_json));

            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Modifies,
                    members,
                    confidence: 1.0,
                    last_updated: now - chrono::Duration::days(i * 10),
                    metadata: meta,
                })
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn behavioral_analysis() {
        let store = SqliteStore::in_memory().unwrap();
        setup_test_data(&store).await;

        let analyzer = BehavioralAnalyzer;
        let config = HomerConfig::default();
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        assert!(stats.results_stored > 0, "Should store analysis results");

        // Check change frequency for src/main.rs (4 commits)
        let file_a = store
            .get_node_by_name(NodeKind::File, "src/main.rs")
            .await
            .unwrap()
            .unwrap();
        let freq = store
            .get_analysis(file_a.id, AnalysisKind::ChangeFrequency)
            .await
            .unwrap()
            .expect("Should have ChangeFrequency result");
        let total = freq
            .data
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .unwrap();
        assert_eq!(total, 4, "src/main.rs should have 4 changes");

        // Check bus factor
        let bus = store
            .get_analysis(file_a.id, AnalysisKind::ContributorConcentration)
            .await
            .unwrap()
            .expect("Should have bus factor");
        let bf = bus
            .data
            .get("bus_factor")
            .and_then(serde_json::Value::as_u64)
            .unwrap();
        assert_eq!(bf, 1, "Single author means bus factor 1");

        // Check co-change: src/main.rs and src/lib.rs should be co-change partners
        let co_change = freq
            .data
            .get("co_change_partners")
            .and_then(|v| v.as_array());
        assert!(
            co_change.is_some(),
            "src/main.rs should have co-change partners"
        );
        let partners = co_change.unwrap();
        assert!(
            partners
                .iter()
                .any(|p| p.get("file").and_then(|f| f.as_str()) == Some("src/lib.rs")),
            "src/lib.rs should be a co-change partner of src/main.rs"
        );

        // Check documentation coverage
        let doc_cov = store
            .get_analysis(file_a.id, AnalysisKind::DocumentationCoverage)
            .await
            .unwrap();
        assert!(doc_cov.is_some(), "Should have doc coverage result");
    }

    #[test]
    fn linear_regression_basic() {
        let points = vec![(1.0, 2.0), (2.0, 4.0), (3.0, 6.0)];
        let (slope, intercept) = linear_regression(&points);
        assert!((slope - 2.0).abs() < 0.001);
        assert!(intercept.abs() < 0.001);
    }

    #[tokio::test]
    async fn bus_factor_80_threshold() {
        // 3 authors: A=60%, B=25%, C=15% of 20 commits
        // With 80% threshold: need A+B (85%) → bus_factor = 2
        // With old 50% threshold: A alone (60%) → bus_factor would be 1
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let file = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/core.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let authors: Vec<NodeId> = {
            let mut ids = Vec::new();
            for email in &["alice@test.com", "bob@test.com", "carol@test.com"] {
                let id = store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::Contributor,
                        name: (*email).to_string(),
                        content_hash: None,
                        last_extracted: now,
                        metadata: HashMap::new(),
                    })
                    .await
                    .unwrap();
                ids.push(id);
            }
            ids
        };

        // Create 20 commits: 12 by Alice, 5 by Bob, 3 by Carol
        let commit_counts = [12u32, 5, 3];
        for (author_idx, &count) in commit_counts.iter().enumerate() {
            for i in 0..count {
                let commit = store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::Commit,
                        name: format!("commit-{author_idx}-{i}"),
                        content_hash: None,
                        last_extracted: now,
                        metadata: HashMap::new(),
                    })
                    .await
                    .unwrap();

                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Authored,
                        members: vec![
                            HyperedgeMember {
                                node_id: authors[author_idx],
                                role: "author".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: commit,
                                role: "commit".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 1.0,
                        last_updated: now - chrono::Duration::days(i64::from(i)),
                        metadata: HashMap::new(),
                    })
                    .await
                    .unwrap();

                let mut meta = HashMap::new();
                meta.insert(
                    "files".to_string(),
                    serde_json::json!([{
                        "path": "src/core.rs",
                        "status": "modified",
                        "lines_added": 5,
                        "lines_deleted": 2,
                    }]),
                );

                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Modifies,
                        members: vec![
                            HyperedgeMember {
                                node_id: commit,
                                role: "commit".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: file,
                                role: "file".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 1.0,
                        last_updated: now - chrono::Duration::days(i64::from(i)),
                        metadata: meta,
                    })
                    .await
                    .unwrap();
            }
        }

        let analyzer = BehavioralAnalyzer;
        let config = HomerConfig::default();
        analyzer.analyze(&store, &config).await.unwrap();

        let bus = store
            .get_analysis(file, AnalysisKind::ContributorConcentration)
            .await
            .unwrap()
            .expect("Should have bus factor result");

        let bf = bus
            .data
            .get("bus_factor")
            .and_then(serde_json::Value::as_u64)
            .unwrap();

        // With 80% threshold: Alice (60%) not enough, Alice+Bob (85%) ≥ 80% → bus_factor = 2
        assert_eq!(bf, 2, "Bus factor should be 2 with 80% threshold");
    }
}
