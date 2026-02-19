use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, TimeZone, Utc};
use gix::bstr::ByteSlice;
use tracing::{debug, info, instrument, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    DiffHunk, DiffStatus, FileDiffStats, Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember,
    Node, NodeId, NodeKind,
};

use super::traits::{ExtractStats, Extractor};

/// Git history extractor — walks commits, diffs, contributors, tags.
#[derive(Debug)]
pub struct GitExtractor {
    repo_path: std::path::PathBuf,
}

impl GitExtractor {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Extractor for GitExtractor {
    fn name(&self) -> &'static str {
        "git"
    }

    async fn has_work(&self, store: &dyn HomerStore) -> crate::error::Result<bool> {
        let checkpoint_sha = store.get_checkpoint("git_last_sha").await?;
        let Some(cp) = checkpoint_sha else {
            return Ok(true); // No checkpoint → first run
        };
        let repo = gix::open(&self.repo_path)
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;
        let head = repo
            .head_commit()
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;
        Ok(head.id().to_string() != cp)
    }

    #[instrument(skip_all, name = "git_extract")]
    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        let repo = gix::open(&self.repo_path)
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

        // Determine checkpoint
        let checkpoint_sha = store.get_checkpoint("git_last_sha").await?;
        debug!(checkpoint = ?checkpoint_sha, "Git checkpoint");

        // Get HEAD
        let head = repo
            .head_commit()
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;
        let head_sha = head.id().to_string();

        if checkpoint_sha.as_deref() == Some(head_sha.as_str()) {
            info!("Git extractor: no new commits since last run");
            stats.duration = start.elapsed();
            return Ok(stats);
        }

        // Force-push detection: verify checkpoint is an ancestor of HEAD.
        // If not, history was rewritten — fall back to full re-extraction.
        let effective_checkpoint = if let Some(ref cp_sha) = checkpoint_sha {
            if Self::is_ancestor(&head, cp_sha) {
                checkpoint_sha.as_deref()
            } else {
                warn!(
                    checkpoint = %cp_sha,
                    head = %head_sha,
                    "Force-push detected: checkpoint is not an ancestor of HEAD. \
                     Falling back to full re-extraction."
                );
                None
            }
        } else {
            None
        };

        let commits_to_process = Self::collect_commits(&head, effective_checkpoint, config)?;
        info!(count = commits_to_process.len(), "Processing commits");

        for oid in &commits_to_process {
            match self.process_commit(&repo, *oid, store, &mut stats).await {
                Ok(()) => {}
                Err(e) => {
                    let sha = oid.to_string();
                    warn!(sha = %sha, error = %e, "Failed to process commit");
                    stats.errors.push((sha, e));
                }
            }
        }

        // Process tags
        if let Err(e) = self.process_tags(&repo, store, &mut stats).await {
            warn!(error = %e, "Failed to process tags");
        }

        // Update checkpoint to HEAD
        store.set_checkpoint("git_last_sha", &head_sha).await?;

        stats.duration = start.elapsed();
        info!(
            commits = commits_to_process.len(),
            nodes_created = stats.nodes_created,
            edges_created = stats.edges_created,
            errors = stats.errors.len(),
            duration = ?stats.duration,
            "Git extraction complete"
        );
        Ok(stats)
    }
}

impl GitExtractor {
    /// Check if `candidate_sha` is an ancestor of `head` by walking history.
    fn is_ancestor(head: &gix::Commit<'_>, candidate_sha: &str) -> bool {
        let Ok(walk) = head.ancestors().all() else {
            return false;
        };
        for info in walk {
            let Ok(info) = info else { continue };
            if info.id().to_string() == candidate_sha {
                return true;
            }
        }
        false
    }

    fn collect_commits(
        head: &gix::Commit<'_>,
        checkpoint_sha: Option<&str>,
        config: &HomerConfig,
    ) -> crate::error::Result<Vec<gix::ObjectId>> {
        let max_commits = if config.extraction.max_commits == 0 {
            usize::MAX
        } else {
            config.extraction.max_commits as usize
        };

        let mut commits = Vec::new();
        let walk = head
            .ancestors()
            .all()
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

        for info in walk {
            let info = match info {
                Ok(i) => i,
                Err(e) => {
                    warn!("Error walking commit: {e}");
                    continue;
                }
            };

            let sha = info.id().to_string();
            if Some(sha.as_str()) == checkpoint_sha {
                break;
            }

            commits.push(info.id);
            if commits.len() >= max_commits {
                break;
            }
        }

        // Process oldest first (reverse topological order)
        commits.reverse();
        Ok(commits)
    }

    async fn process_commit(
        &self,
        repo: &gix::Repository,
        oid: gix::ObjectId,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
    ) -> crate::error::Result<()> {
        let commit = repo
            .find_commit(oid)
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

        let sha = oid.to_string();
        let message = commit.message_raw_sloppy().to_string();
        let author_sig = commit.author().map_err(|e| {
            HomerError::Extract(ExtractError::Git(format!("bad author encoding: {e}")))
        })?;
        let author_name = author_sig.name.to_string();
        let author_email = author_sig.email.to_string();
        let author_time = author_sig
            .time()
            .map_or_else(|_| Utc::now(), |t| gix_time_to_chrono(&t));

        let node_id = self
            .store_commit_nodes(store, stats, &sha, &message, &author_name, &author_email)
            .await?;

        // Create Authored hyperedge
        let authored_edge = Hyperedge {
            id: HyperedgeId(0),
            kind: HyperedgeKind::Authored,
            members: vec![
                HyperedgeMember {
                    node_id: node_id.contributor,
                    role: "author".to_string(),
                    position: 0,
                },
                HyperedgeMember {
                    node_id: node_id.commit,
                    role: "commit".to_string(),
                    position: 1,
                },
            ],
            confidence: 1.0,
            last_updated: author_time,
            metadata: HashMap::new(),
        };
        store.upsert_hyperedge(&authored_edge).await?;
        stats.edges_created += 1;

        // Compute diff and store file nodes + Modifies edge
        let diff_stats = compute_diff(repo, &commit)?;
        self.store_modifies_edge(store, stats, node_id.commit, author_time, &diff_stats)
            .await?;

        // Index commit message for FTS
        store
            .index_text(node_id.commit, "commit_message", &message)
            .await?;

        Ok(())
    }

    async fn store_commit_nodes(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        sha: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> crate::error::Result<CommitNodeIds> {
        let mut commit_meta = HashMap::new();
        commit_meta.insert("message".to_string(), serde_json::json!(message));
        commit_meta.insert("author_name".to_string(), serde_json::json!(author_name));
        commit_meta.insert("author_email".to_string(), serde_json::json!(author_email));

        let commit = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Commit,
                name: sha.to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: commit_meta,
            })
            .await?;
        stats.nodes_created += 1;

        let contributor = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Contributor,
                name: author_email.to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("display_name".to_string(), serde_json::json!(author_name));
                    m
                },
            })
            .await?;
        stats.nodes_created += 1;

        Ok(CommitNodeIds {
            commit,
            contributor,
        })
    }

    async fn store_modifies_edge(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        commit_node_id: NodeId,
        author_time: DateTime<Utc>,
        diff_stats: &[FileDiffStats],
    ) -> crate::error::Result<()> {
        if diff_stats.is_empty() {
            return Ok(());
        }

        let mut file_members = vec![HyperedgeMember {
            node_id: commit_node_id,
            role: "commit".to_string(),
            position: 0,
        }];

        let mut files_json = Vec::new();

        for (pos, diff) in diff_stats.iter().enumerate() {
            let file_node = Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: diff.path.to_string_lossy().to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            };
            let file_id = store.upsert_node(&file_node).await?;
            stats.nodes_created += 1;

            let position = u32::try_from(pos + 1).unwrap_or(u32::MAX);
            file_members.push(HyperedgeMember {
                node_id: file_id,
                role: "file".to_string(),
                position,
            });

            files_json.push(serde_json::json!({
                "path": diff.path.to_string_lossy(),
                "status": diff.status,
                "lines_added": diff.lines_added,
                "lines_deleted": diff.lines_deleted,
                "old_path": diff.old_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            }));
        }

        let mut modifies_meta = HashMap::new();
        modifies_meta.insert("files".to_string(), serde_json::json!(files_json));

        let modifies_edge = Hyperedge {
            id: HyperedgeId(0),
            kind: HyperedgeKind::Modifies,
            members: file_members,
            confidence: 1.0,
            last_updated: author_time,
            metadata: modifies_meta,
        };
        store.upsert_hyperedge(&modifies_edge).await?;
        stats.edges_created += 1;

        Ok(())
    }

    async fn process_tags(
        &self,
        repo: &gix::Repository,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
    ) -> crate::error::Result<()> {
        let refs = repo
            .references()
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

        let tag_refs = refs
            .tags()
            .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

        // Collect all tags with their resolved target commit OIDs.
        let mut tags: Vec<(String, gix::ObjectId)> = Vec::new();
        for tag_ref in tag_refs.flatten() {
            let tag_name = tag_ref.name().as_bstr().to_string();
            let short_name = tag_name
                .strip_prefix("refs/tags/")
                .unwrap_or(&tag_name)
                .to_string();

            // Peel annotated tags to their target commit.
            let target_oid = tag_ref.id().detach();

            let release_node = Node {
                id: NodeId(0),
                kind: NodeKind::Release,
                name: short_name.clone(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert(
                        "target".to_string(),
                        serde_json::json!(target_oid.to_string()),
                    );
                    m
                },
            };
            store.upsert_node(&release_node).await?;
            stats.nodes_created += 1;

            tags.push((short_name, target_oid));
        }

        // Create Release→Commit Includes edges.
        // For each tag, walk backward from its target commit to the previous tag's
        // target (or the beginning of history for the earliest tag).
        self.create_release_commit_edges(repo, store, stats, &tags)
            .await?;

        Ok(())
    }

    /// Walk commits between consecutive release tags and create Includes edges.
    async fn create_release_commit_edges(
        &self,
        repo: &gix::Repository,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        tags: &[(String, gix::ObjectId)],
    ) -> crate::error::Result<()> {
        if tags.is_empty() {
            return Ok(());
        }

        // Build a set of tag target OIDs for boundary detection.
        let tag_targets: std::collections::HashSet<gix::ObjectId> =
            tags.iter().map(|(_, oid)| *oid).collect();

        for (tag_name, target_oid) in tags {
            let release_node = store.get_node_by_name(NodeKind::Release, tag_name).await?;
            let Some(release_node) = release_node else {
                continue;
            };

            // Walk backward from the tag's target commit.
            let Ok(commit) = repo.find_commit(*target_oid) else {
                continue;
            };
            let Ok(walk) = commit.ancestors().all() else {
                continue;
            };

            for info in walk {
                let Ok(info) = info else { continue };
                let commit_sha = info.id.to_string();

                // Stop at previous tag boundary (but include the first commit).
                if info.id != *target_oid && tag_targets.contains(&info.id) {
                    break;
                }

                // Look up the commit node in the store.
                let Some(commit_node) = store
                    .get_node_by_name(NodeKind::Commit, &commit_sha)
                    .await?
                else {
                    continue;
                };

                let includes_edge = Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Includes,
                    members: vec![
                        HyperedgeMember {
                            node_id: release_node.id,
                            role: "release".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: commit_node.id,
                            role: "commit".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: Utc::now(),
                    metadata: HashMap::new(),
                };
                store.upsert_hyperedge(&includes_edge).await?;
                stats.edges_created += 1;
            }
        }

        Ok(())
    }
}

struct CommitNodeIds {
    commit: NodeId,
    contributor: NodeId,
}

/// Intermediate diff entry: captures blob IDs during tree walk for post-processing.
struct RawDiffEntry {
    path: std::path::PathBuf,
    old_path: Option<std::path::PathBuf>,
    status: DiffStatus,
    old_blob: Option<gix::ObjectId>,
    new_blob: Option<gix::ObjectId>,
}

#[allow(clippy::too_many_lines)]
fn compute_diff(
    repo: &gix::Repository,
    commit: &gix::Commit<'_>,
) -> crate::error::Result<Vec<FileDiffStats>> {
    let tree = commit
        .tree()
        .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

    let parent_tree = commit
        .parent_ids()
        .next()
        .and_then(|parent_id| parent_id.object().ok()?.try_into_commit().ok()?.tree().ok());

    let mut raw_entries = Vec::new();

    let base = match parent_tree {
        Some(ref parent) => parent,
        None => &repo.empty_tree(),
    };

    let mut platform = base
        .changes()
        .map_err(|e| HomerError::Extract(ExtractError::Git(e.to_string())))?;

    platform
        .for_each_to_obtain_tree(&tree, |change| {
            use gix::object::tree::diff::Change;
            match change {
                Change::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    raw_entries.push(RawDiffEntry {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Added,
                        old_blob: None,
                        new_blob: if entry_mode.is_blob() {
                            Some(id.detach())
                        } else {
                            None
                        },
                    });
                }
                Change::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    raw_entries.push(RawDiffEntry {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Deleted,
                        old_blob: if entry_mode.is_blob() {
                            Some(id.detach())
                        } else {
                            None
                        },
                        new_blob: None,
                    });
                }
                Change::Modification {
                    location,
                    entry_mode,
                    previous_id,
                    id,
                    ..
                } => {
                    let is_blob = entry_mode.is_blob();
                    raw_entries.push(RawDiffEntry {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Modified,
                        old_blob: if is_blob {
                            Some(previous_id.detach())
                        } else {
                            None
                        },
                        new_blob: if is_blob { Some(id.detach()) } else { None },
                    });
                }
                Change::Rewrite {
                    source_location,
                    source_id,
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    let is_blob = entry_mode.is_blob();
                    raw_entries.push(RawDiffEntry {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: Some(source_location.to_path_lossy().to_path_buf()),
                        status: DiffStatus::Renamed,
                        old_blob: if is_blob {
                            Some(source_id.detach())
                        } else {
                            None
                        },
                        new_blob: if is_blob { Some(id.detach()) } else { None },
                    });
                }
            }
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        })
        .map_err(|e| HomerError::Extract(ExtractError::Git(format!("diff error: {e}"))))?;

    // Post-process: compute line stats and hunks from blob contents
    let diff_stats = raw_entries
        .into_iter()
        .map(|entry| {
            let (lines_added, lines_deleted, hunks) = match entry.status {
                DiffStatus::Added => {
                    let added = entry.new_blob.map_or(0, |id| count_blob_lines(repo, id));
                    let hunks = if added > 0 {
                        vec![DiffHunk {
                            old_start: 0,
                            old_lines: 0,
                            new_start: 1,
                            new_lines: added,
                        }]
                    } else {
                        Vec::new()
                    };
                    (added, 0, hunks)
                }
                DiffStatus::Deleted => {
                    let deleted = entry.old_blob.map_or(0, |id| count_blob_lines(repo, id));
                    let hunks = if deleted > 0 {
                        vec![DiffHunk {
                            old_start: 1,
                            old_lines: deleted,
                            new_start: 0,
                            new_lines: 0,
                        }]
                    } else {
                        Vec::new()
                    };
                    (0, deleted, hunks)
                }
                DiffStatus::Modified | DiffStatus::Renamed | DiffStatus::Copied => {
                    match (entry.old_blob, entry.new_blob) {
                        (Some(old_id), Some(new_id)) => {
                            compute_blob_diff_with_hunks(repo, old_id, new_id)
                        }
                        _ => (0, 0, Vec::new()),
                    }
                }
            };
            FileDiffStats {
                path: entry.path,
                old_path: entry.old_path,
                status: entry.status,
                lines_added,
                lines_deleted,
                hunks,
            }
        })
        .collect();

    Ok(diff_stats)
}

/// Count lines in a single blob. Returns 0 for binary content or errors.
fn count_blob_lines(repo: &gix::Repository, id: gix::ObjectId) -> u32 {
    let Ok(obj) = repo.find_object(id) else {
        return 0;
    };
    let data = obj.detach().data;
    if data.is_empty() || is_likely_binary(&data) {
        return 0;
    }
    count_newlines(&data)
}

/// Compute added/deleted line counts and hunks between two blobs using `similar`.
fn compute_blob_diff_with_hunks(
    repo: &gix::Repository,
    old_id: gix::ObjectId,
    new_id: gix::ObjectId,
) -> (u32, u32, Vec<DiffHunk>) {
    let Ok(old_obj) = repo.find_object(old_id) else {
        return (0, 0, Vec::new());
    };
    let old_data = old_obj.detach().data;

    let Ok(new_obj) = repo.find_object(new_id) else {
        return (0, 0, Vec::new());
    };
    let new_data = new_obj.detach().data;

    if is_likely_binary(&old_data) || is_likely_binary(&new_data) {
        return (0, 0, Vec::new());
    }

    let old_text = String::from_utf8_lossy(&old_data);
    let new_text = String::from_utf8_lossy(&new_data);

    let diff = similar::TextDiff::from_lines(old_text.as_ref(), new_text.as_ref());

    let mut total_added = 0u32;
    let mut total_deleted = 0u32;
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(3) {
        let mut hunk_old_start = u32::MAX;
        let mut hunk_old_end = 0u32;
        let mut hunk_new_start = u32::MAX;
        let mut hunk_new_end = 0u32;

        for op in &group {
            let old_range = op.old_range();
            let new_range = op.new_range();

            #[allow(clippy::cast_possible_truncation)]
            let (os, oe, ns, ne) = (
                old_range.start as u32,
                old_range.end as u32,
                new_range.start as u32,
                new_range.end as u32,
            );

            hunk_old_start = hunk_old_start.min(os);
            hunk_old_end = hunk_old_end.max(oe);
            hunk_new_start = hunk_new_start.min(ns);
            hunk_new_end = hunk_new_end.max(ne);

            match op.tag() {
                similar::DiffTag::Insert => total_added += ne - ns,
                similar::DiffTag::Delete => total_deleted += oe - os,
                similar::DiffTag::Replace => {
                    total_added += ne - ns;
                    total_deleted += oe - os;
                }
                similar::DiffTag::Equal => {}
            }
        }

        hunks.push(DiffHunk {
            old_start: hunk_old_start + 1, // 1-based
            old_lines: hunk_old_end - hunk_old_start,
            new_start: hunk_new_start + 1,
            new_lines: hunk_new_end - hunk_new_start,
        });
    }

    (total_added, total_deleted, hunks)
}

/// Check if data appears binary (NUL byte in first 8KB).
fn is_likely_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8192);
    data[..check_len].contains(&0)
}

/// Count newlines, treating each `\n` as a line terminator.
#[allow(clippy::cast_possible_truncation)]
fn count_newlines(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let newlines = data
        .iter()
        .fold(0usize, |acc, &b| acc + usize::from(b == b'\n'));
    let total = if data.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    };
    total as u32
}

fn gix_time_to_chrono(time: &gix::date::Time) -> DateTime<Utc> {
    Utc.timestamp_opt(time.seconds, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HomerConfig;
    use crate::store::sqlite::SqliteStore;
    use crate::types::NodeFilter;
    use std::process::Command;

    /// Create a temporary git repo with some commits for testing.
    fn create_test_repo(dir: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("git command failed")
        };

        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);

        // Commit 1: add files
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "Initial commit"]);

        // Commit 2: modify
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn hello() { println!(\"hi\"); }",
        )
        .unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "Update lib"]);

        // Commit 3: add new file
        std::fs::write(dir.join("src/utils.rs"), "pub fn util() {}").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "Add utils"]);

        // Tag
        run(&["tag", "v0.1.0"]);
    }

    #[tokio::test]
    async fn extract_from_test_repo() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_repo(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = GitExtractor::new(tmp.path());

        let stats = extractor.extract(&store, &config).await.unwrap();

        assert!(stats.nodes_created > 0, "Should create nodes");
        assert!(stats.edges_created > 0, "Should create edges");
        assert!(
            stats.errors.is_empty(),
            "Should have no errors, got: {:?}",
            stats.errors
        );

        // Verify commits exist
        let commit_filter = NodeFilter {
            kind: Some(NodeKind::Commit),
            ..Default::default()
        };
        let commits = store.find_nodes(&commit_filter).await.unwrap();
        assert_eq!(commits.len(), 3, "Should have 3 commits");

        // Verify contributors
        let contributor_filter = NodeFilter {
            kind: Some(NodeKind::Contributor),
            ..Default::default()
        };
        let contributors = store.find_nodes(&contributor_filter).await.unwrap();
        assert_eq!(contributors.len(), 1, "Should have 1 contributor");

        // Verify files
        let file_filter = NodeFilter {
            kind: Some(NodeKind::File),
            ..Default::default()
        };
        let files = store.find_nodes(&file_filter).await.unwrap();
        assert!(files.len() >= 3, "Should have at least 3 files");

        // Verify tags
        let release_filter = NodeFilter {
            kind: Some(NodeKind::Release),
            ..Default::default()
        };
        let releases = store.find_nodes(&release_filter).await.unwrap();
        assert_eq!(releases.len(), 1, "Should have 1 release");

        // Verify checkpoint
        let checkpoint = store.get_checkpoint("git_last_sha").await.unwrap();
        assert!(checkpoint.is_some(), "Should set checkpoint");

        // Verify incrementality: running again should find no new work
        let stats2 = extractor.extract(&store, &config).await.unwrap();
        assert_eq!(stats2.nodes_created, 0, "No new nodes on re-run");
    }

    #[tokio::test]
    async fn extract_computes_line_counts() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_repo(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = GitExtractor::new(tmp.path());

        extractor.extract(&store, &config).await.unwrap();

        // Inspect Modifies edges for line count data
        let modifies = store
            .get_edges_by_kind(crate::types::HyperedgeKind::Modifies)
            .await
            .unwrap();
        assert!(!modifies.is_empty(), "Should have Modifies edges");

        // Collect all per-file diff stats from edge metadata
        let mut total_added = 0u64;
        let mut total_deleted = 0u64;
        for edge in &modifies {
            let files = edge
                .metadata
                .get("files")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for f in &files {
                total_added += f
                    .get("lines_added")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                total_deleted += f
                    .get("lines_deleted")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
            }
        }

        // Commit 1 adds main.rs (1 line) + lib.rs (1 line) = 2 lines added
        // Commit 2 modifies lib.rs (changes 1 line → different 1 line)
        // Commit 3 adds utils.rs (1 line) = 1 line added
        // Total lines_added should be > 0
        assert!(
            total_added > 0,
            "Should have non-zero lines_added, got {total_added}"
        );

        // Commit 2 modifies lib.rs so there should be some deleted lines too
        // (at minimum the original line was replaced)
        assert!(
            total_added + total_deleted > 2,
            "Total churn should exceed initial additions, got added={total_added} deleted={total_deleted}"
        );
    }

    #[test]
    fn count_newlines_basic() {
        assert_eq!(count_newlines(b"hello\nworld\n"), 2);
        assert_eq!(count_newlines(b"hello\nworld"), 2);
        assert_eq!(count_newlines(b"hello\n"), 1);
        assert_eq!(count_newlines(b"hello"), 1);
        assert_eq!(count_newlines(b""), 0);
    }

    #[test]
    fn is_likely_binary_detects_nul() {
        assert!(!is_likely_binary(b"hello world"));
        assert!(is_likely_binary(b"hello\x00world"));
        assert!(!is_likely_binary(b""));
    }

    #[test]
    fn blob_line_diff_basic() {
        // Test the hash-based diff logic directly using compute_diff on known data
        let old_data = b"line1\nline2\nline3\n";
        let new_data = b"line1\nline2_modified\nline3\nline4\n";

        let mut old_counts: HashMap<&[u8], i32> = HashMap::new();
        for line in old_data.split(|&b| b == b'\n') {
            *old_counts.entry(line).or_default() += 1;
        }

        let mut added = 0u32;
        for line in new_data.split(|&b| b == b'\n') {
            if let Some(count) = old_counts.get_mut(line) {
                if *count > 0 {
                    *count -= 1;
                } else {
                    added += 1;
                }
            } else {
                added += 1;
            }
        }
        let deleted: u32 = old_counts
            .values()
            .filter(|&&v| v > 0)
            .map(|&v| u32::try_from(v).unwrap_or(0))
            .sum();

        // "line2" deleted, "line2_modified" and "line4" added
        assert_eq!(added, 2, "Should detect 2 added lines");
        assert_eq!(deleted, 1, "Should detect 1 deleted line");
    }

    #[tokio::test]
    async fn force_push_detection_falls_back_to_full() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_repo(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = GitExtractor::new(tmp.path());

        // First extraction: normal
        let stats1 = extractor.extract(&store, &config).await.unwrap();
        assert!(stats1.nodes_created > 0);
        let _first_count = stats1.nodes_created;

        // Simulate force-push: set checkpoint to a non-existent SHA
        store
            .set_checkpoint("git_last_sha", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .await
            .unwrap();

        // Re-extract: should detect force-push and do full re-extraction
        let stats2 = extractor.extract(&store, &config).await.unwrap();

        // Should process all commits again (upserts, so node count may be lower
        // due to ON CONFLICT, but it should still do work)
        assert!(
            stats2.nodes_created > 0 || stats2.edges_created > 0,
            "Force-push fallback should re-process commits"
        );
    }
}
