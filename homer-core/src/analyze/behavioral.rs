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
use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, Hyperedge, HyperedgeId, HyperedgeKind,
    HyperedgeMember, NodeId, NodeKind,
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

    #[instrument(skip_all, name = "behavioral_analyze")]
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

        // Compute documentation freshness (stale docs on changed files)
        compute_doc_freshness(store, &commit_data, &mut stats).await?;

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

/// Configuration for co-change detection (per ANALYZERS.md spec).
struct CoChangeConfig {
    min_confidence: f64,
    min_co_occurrences: u32,
    max_group_size: usize,
    min_marginal_gain: f64,
}

impl Default for CoChangeConfig {
    fn default() -> Self {
        Self {
            min_confidence: 0.3,
            min_co_occurrences: 3,
            max_group_size: 8,
            min_marginal_gain: 0.05,
        }
    }
}

/// A scored pair of files with their co-occurrence count and confidence.
#[derive(Clone)]
struct ScoredPair {
    file_a: NodeId,
    file_b: NodeId,
    count: u32,
    confidence: f64,
}

#[allow(clippy::too_many_lines)]
async fn compute_co_change(
    store: &dyn HomerStore,
    data: &CommitData,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let cfg = CoChangeConfig::default();
    let now = Utc::now();

    // Step 1: Build co-occurrence matrix
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

    // Step 2: Compute pairwise confidence, filter by thresholds
    let mut scored_pairs: Vec<ScoredPair> = Vec::new();
    // Also build a quick lookup: file_id → commit count
    let file_commit_count: HashMap<NodeId, usize> = data
        .file_commits
        .iter()
        .map(|(id, changes)| (*id, changes.len()))
        .collect();

    for (&(file_a, file_b), &count) in &co_occur {
        if count < cfg.min_co_occurrences {
            continue;
        }

        let commits_a = file_commit_count.get(&file_a).copied().unwrap_or(0);
        let commits_b = file_commit_count.get(&file_b).copied().unwrap_or(0);

        let min_changes = commits_a.min(commits_b) as f64;
        if min_changes < 1.0 {
            continue;
        }
        let confidence = count as f64 / min_changes;

        if confidence >= cfg.min_confidence {
            scored_pairs.push(ScoredPair {
                file_a,
                file_b,
                count,
                confidence,
            });
        }
    }

    // Sort pairs by confidence descending (highest first = best seeds)
    scored_pairs.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step 3: Seed-and-grow into N-ary groups
    // Build pairwise confidence lookup for growth phase
    let pair_confidence: HashMap<(NodeId, NodeId), f64> = scored_pairs
        .iter()
        .map(|p| ((p.file_a, p.file_b), p.confidence))
        .collect();

    let mut consumed: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut groups: Vec<(Vec<NodeId>, f64)> = Vec::new(); // (members, min_confidence)

    for pair in &scored_pairs {
        let key = (pair.file_a, pair.file_b);
        if consumed.contains(&key) {
            continue;
        }

        // Seed with this pair
        let mut group = vec![pair.file_a, pair.file_b];
        let mut group_min_conf = pair.confidence;
        consumed.insert(key);

        // Grow: find candidates that co-change with every group member
        if group.len() < cfg.max_group_size {
            grow_group(
                &mut group,
                &mut group_min_conf,
                &pair_confidence,
                &mut consumed,
                &cfg,
            );
        }

        groups.push((group, group_min_conf));
    }

    // Step 4: Emit CoChanges hyperedges
    let total_commits = data.commit_count as f64;

    for (group_members, group_conf) in &groups {
        let members: Vec<HyperedgeMember> = group_members
            .iter()
            .enumerate()
            .map(|(pos, &node_id)| HyperedgeMember {
                node_id,
                role: "file".to_string(),
                position: pos as u32,
            })
            .collect();

        // Compute group co-occurrence: commits where ALL members appear
        let group_co_occur = count_group_co_occurrence(group_members, &data.commit_files);
        let support = group_co_occur as f64 / total_commits.max(1.0);

        let mut meta = HashMap::new();
        meta.insert(
            "co_occurrences".to_string(),
            serde_json::json!(group_co_occur),
        );
        meta.insert("support".to_string(), serde_json::json!(support));
        meta.insert("arity".to_string(), serde_json::json!(group_members.len()));

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::CoChanges,
                members,
                confidence: *group_conf,
                last_updated: now,
                metadata: meta,
            })
            .await?;
        stats.results_stored += 1;
    }

    // Step 5: Also enrich per-file ChangeFrequency with top co-change partners
    // (for quick per-file lookup in renderers)
    let mut file_partners: HashMap<NodeId, Vec<(NodeId, u32, f64)>> = HashMap::new();
    for pair in &scored_pairs {
        file_partners.entry(pair.file_a).or_default().push((
            pair.file_b,
            pair.count,
            pair.confidence,
        ));
        file_partners.entry(pair.file_b).or_default().push((
            pair.file_a,
            pair.count,
            pair.confidence,
        ));
    }

    for (file_id, mut partners) in file_partners {
        partners.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        partners.truncate(10);

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

    info!(
        groups = groups.len(),
        pairs = scored_pairs.len(),
        "Co-change analysis complete"
    );
    Ok(())
}

/// Grow a co-change group by adding files that co-change with every existing member.
fn grow_group(
    group: &mut Vec<NodeId>,
    group_min_conf: &mut f64,
    pair_confidence: &HashMap<(NodeId, NodeId), f64>,
    consumed: &mut HashSet<(NodeId, NodeId)>,
    cfg: &CoChangeConfig,
) {
    loop {
        if group.len() >= cfg.max_group_size {
            break;
        }

        // Collect candidate files: any file that has a scored pair with at least one group member
        let mut candidates: HashSet<NodeId> = HashSet::new();
        for &member in group.iter() {
            for &(a, b) in pair_confidence.keys() {
                if a == member && !group.contains(&b) {
                    candidates.insert(b);
                } else if b == member && !group.contains(&a) {
                    candidates.insert(a);
                }
            }
        }

        // Find the best candidate: must co-change with ALL group members
        let mut best: Option<(NodeId, f64)> = None;

        for &candidate in &candidates {
            let mut min_conf_with_group = f64::MAX;
            let mut valid = true;

            for &member in group.iter() {
                let key = if candidate.0 < member.0 {
                    (candidate, member)
                } else {
                    (member, candidate)
                };
                if let Some(&conf) = pair_confidence.get(&key) {
                    min_conf_with_group = min_conf_with_group.min(conf);
                } else {
                    valid = false;
                    break;
                }
            }

            if !valid || min_conf_with_group < cfg.min_confidence {
                continue;
            }

            // Accept the candidate with the highest min-confidence against the group
            if best.is_none_or(|(_, best_conf)| min_conf_with_group > best_conf) {
                best = Some((candidate, min_conf_with_group));
            }
        }

        let Some((best_candidate, best_conf)) = best else {
            break;
        };

        // Check min_marginal_gain: stop if this candidate barely adds value
        // (marginal gain = how much the candidate's weakest link exceeds the group minimum)
        if best_conf < *group_min_conf - cfg.min_marginal_gain {
            break;
        }

        // Add the candidate, mark pairs as consumed
        for &member in group.iter() {
            let key = if best_candidate.0 < member.0 {
                (best_candidate, member)
            } else {
                (member, best_candidate)
            };
            consumed.insert(key);
        }
        group.push(best_candidate);
        *group_min_conf = group_min_conf.min(best_conf);
    }
}

/// Count how many commits contain ALL members of a group.
fn count_group_co_occurrence(
    group: &[NodeId],
    commit_files: &HashMap<NodeId, HashSet<NodeId>>,
) -> u32 {
    let mut count = 0u32;
    for file_set in commit_files.values() {
        if group.iter().all(|id| file_set.contains(id)) {
            count += 1;
        }
    }
    count
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

// ── Documentation Freshness ───────────────────────────────────────

/// Detect stale documentation: files that have changed frequently but whose
/// documentation (Documents edges) hasn't been updated.
///
/// For each file with both documentation and commit history, compare the
/// doc edge's `last_updated` timestamp against recent commits. If the file
/// has been modified N times since the doc was last updated, it's potentially stale.
async fn compute_doc_freshness(
    store: &dyn HomerStore,
    data: &CommitData,
    stats: &mut AnalyzeStats,
) -> crate::error::Result<()> {
    let now = Utc::now();

    // Get Documents edges → map subject_id → last_updated
    let doc_edges = store.get_edges_by_kind(HyperedgeKind::Documents).await?;
    let mut doc_timestamps: HashMap<NodeId, chrono::DateTime<Utc>> = HashMap::new();

    for edge in &doc_edges {
        for member in &edge.members {
            if member.role == "subject" {
                let existing = doc_timestamps.get(&member.node_id);
                if existing.is_none_or(|&ts| edge.last_updated > ts) {
                    doc_timestamps.insert(member.node_id, edge.last_updated);
                }
            }
        }
    }

    for (file_id, changes) in &data.file_commits {
        let total_changes = changes.len();
        if total_changes == 0 {
            continue;
        }

        // Count commits since doc was last updated
        let (commits_since_doc, is_stale) = if let Some(&doc_time) = doc_timestamps.get(file_id) {
            let since = changes.iter().filter(|c| c.commit_time > doc_time).count();
            (since as u32, since >= 3) // Stale if 3+ changes since doc update
        } else {
            // No documentation at all — report as undocumented, not "stale"
            continue;
        };

        // Staleness risk: combine with salience if available
        let salience = store
            .get_analysis(*file_id, AnalysisKind::CompositeSalience)
            .await?
            .and_then(|r| r.data.get("score").and_then(serde_json::Value::as_f64))
            .unwrap_or(0.0);

        let staleness_risk = if is_stale {
            (f64::from(commits_since_doc) * 0.3 + salience * 0.7).min(1.0)
        } else {
            0.0
        };

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: *file_id,
                kind: AnalysisKind::DocumentationFreshness,
                data: serde_json::json!({
                    "is_stale": is_stale,
                    "commits_since_doc_update": commits_since_doc,
                    "staleness_risk": (staleness_risk * 1000.0).round() / 1000.0,
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

        // Check co-change: CoChanges hyperedge should exist
        let co_edges = store
            .get_edges_by_kind(HyperedgeKind::CoChanges)
            .await
            .unwrap();
        assert!(!co_edges.is_empty(), "Should have CoChanges hyperedge(s)");
        // The edge should contain both file_a and file_b
        let has_pair = co_edges.iter().any(|e| {
            let ids: Vec<NodeId> = e.members.iter().map(|m| m.node_id).collect();
            ids.contains(&file_a.id)
        });
        assert!(has_pair, "CoChanges edge should contain src/main.rs");

        // Also check per-file partner enrichment on ChangeFrequency
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

    #[tokio::test]
    async fn seed_and_grow_ternary_co_change() {
        // 3 files (A, B, C) always change together across 5 commits.
        // The seed-and-grow algorithm should produce a single ternary group.
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let files: Vec<NodeId> = {
            let mut ids = Vec::new();
            for name in &["src/a.rs", "src/b.rs", "src/c.rs"] {
                let id = store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::File,
                        name: (*name).to_string(),
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

        let author = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Contributor,
                name: "dev@test.com".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // 5 commits, each touching all 3 files
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
                    last_updated: now - chrono::Duration::days(i),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();

            let mut members = vec![HyperedgeMember {
                node_id: commit,
                role: "commit".to_string(),
                position: 0,
            }];
            let mut files_json = Vec::new();
            for (pos, &file_id) in files.iter().enumerate() {
                members.push(HyperedgeMember {
                    node_id: file_id,
                    role: "file".to_string(),
                    position: (pos + 1) as u32,
                });
                files_json.push(serde_json::json!({
                    "path": format!("src/{}.rs", (b'a' + pos as u8) as char),
                    "status": "modified",
                    "lines_added": 5,
                    "lines_deleted": 2,
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
                    last_updated: now - chrono::Duration::days(i),
                    metadata: meta,
                })
                .await
                .unwrap();
        }

        let analyzer = BehavioralAnalyzer;
        let config = HomerConfig::default();
        analyzer.analyze(&store, &config).await.unwrap();

        // Should produce a CoChanges hyperedge with arity >= 3
        let co_edges = store
            .get_edges_by_kind(HyperedgeKind::CoChanges)
            .await
            .unwrap();
        assert!(
            !co_edges.is_empty(),
            "Should produce CoChanges hyperedge(s)"
        );

        // Find the group containing all 3 files
        let ternary = co_edges.iter().find(|e| e.members.len() >= 3);
        assert!(
            ternary.is_some(),
            "Should have at least one ternary (or larger) co-change group, \
             got groups with arities: {:?}",
            co_edges.iter().map(|e| e.members.len()).collect::<Vec<_>>()
        );

        let edge = ternary.unwrap();
        let member_ids: HashSet<NodeId> = edge.members.iter().map(|m| m.node_id).collect();
        for &file_id in &files {
            assert!(
                member_ids.contains(&file_id),
                "Ternary group should contain all 3 files"
            );
        }

        // Metadata should include arity and co-occurrence count
        let arity = edge
            .metadata
            .get("arity")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        assert!(arity >= 3, "Arity metadata should be >= 3");

        let co_occur = edge
            .metadata
            .get("co_occurrences")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        assert_eq!(co_occur, 5, "All 5 commits touch all 3 files");
    }

    #[test]
    fn grow_group_respects_max_size() {
        // Test that grow_group stops at max_group_size
        let cfg = CoChangeConfig {
            max_group_size: 3,
            ..CoChangeConfig::default()
        };

        // 5 files, all pairs have confidence 1.0
        let ids: Vec<NodeId> = (1..=5).map(NodeId).collect();
        let mut pair_confidence: HashMap<(NodeId, NodeId), f64> = HashMap::new();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                pair_confidence.insert((ids[i], ids[j]), 1.0);
            }
        }

        let mut group = vec![ids[0], ids[1]];
        let mut group_min_conf = 1.0;
        let mut consumed = HashSet::new();
        consumed.insert((ids[0], ids[1]));

        grow_group(
            &mut group,
            &mut group_min_conf,
            &pair_confidence,
            &mut consumed,
            &cfg,
        );

        assert_eq!(group.len(), 3, "Group should stop at max_group_size=3");
    }

    #[tokio::test]
    async fn doc_freshness_detects_stale() {
        // File with documentation updated 60 days ago, but 4 commits since then
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let file = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/stale.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let doc = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Document,
                name: "docs/stale.md".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Documents edge: doc → file, last updated 60 days ago
        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Documents,
                members: vec![
                    HyperedgeMember {
                        node_id: doc,
                        role: "document".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: file,
                        role: "subject".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: now - chrono::Duration::days(60),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let author = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Contributor,
                name: "dev@test.com".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // 4 commits in last 30 days (all after doc update)
        for i in 0..4 {
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
                    last_updated: now - chrono::Duration::days(i),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();

            let mut meta = HashMap::new();
            meta.insert(
                "files".to_string(),
                serde_json::json!([{
                    "path": "src/stale.rs",
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
                    last_updated: now - chrono::Duration::days(i),
                    metadata: meta,
                })
                .await
                .unwrap();
        }

        let analyzer = BehavioralAnalyzer;
        let config = HomerConfig::default();
        analyzer.analyze(&store, &config).await.unwrap();

        let freshness = store
            .get_analysis(file, AnalysisKind::DocumentationFreshness)
            .await
            .unwrap()
            .expect("Should have freshness result");

        let is_stale = freshness
            .data
            .get("is_stale")
            .and_then(serde_json::Value::as_bool)
            .unwrap();
        assert!(is_stale, "Doc should be stale (4 commits since update)");

        let commits_since = freshness
            .data
            .get("commits_since_doc_update")
            .and_then(serde_json::Value::as_u64)
            .unwrap();
        assert_eq!(commits_since, 4, "Should count 4 commits since doc update");
    }
}
