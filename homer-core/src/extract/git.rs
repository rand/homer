use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, TimeZone, Utc};
use gix::bstr::ByteSlice;
use tracing::{debug, info, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    DiffStatus, FileDiffStats, Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node,
    NodeId, NodeKind,
};

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

    /// Run full git extraction into the store.
    pub async fn extract(
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

        let commits_to_process = Self::collect_commits(&head, checkpoint_sha.as_deref(), config)?;
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

        for tag_ref in tag_refs.flatten() {
            let tag_name = tag_ref.name().as_bstr().to_string();
            let short_name = tag_name.strip_prefix("refs/tags/").unwrap_or(&tag_name);

            let release_node = Node {
                id: NodeId(0),
                kind: NodeKind::Release,
                name: short_name.to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert(
                        "target".to_string(),
                        serde_json::json!(tag_ref.id().to_string()),
                    );
                    m
                },
            };
            store.upsert_node(&release_node).await?;
            stats.nodes_created += 1;
        }

        Ok(())
    }
}

struct CommitNodeIds {
    commit: NodeId,
    contributor: NodeId,
}

/// Stats from running the extractor.
#[derive(Debug, Default)]
pub struct ExtractStats {
    pub nodes_created: u64,
    pub nodes_updated: u64,
    pub edges_created: u64,
    pub duration: std::time::Duration,
    pub errors: Vec<(String, HomerError)>,
}

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

    let mut diff_stats = Vec::new();

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
                Change::Addition { location, .. } => {
                    diff_stats.push(FileDiffStats {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Added,
                        lines_added: 0,
                        lines_deleted: 0,
                    });
                }
                Change::Deletion { location, .. } => {
                    diff_stats.push(FileDiffStats {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Deleted,
                        lines_added: 0,
                        lines_deleted: 0,
                    });
                }
                Change::Modification { location, .. } => {
                    diff_stats.push(FileDiffStats {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: None,
                        status: DiffStatus::Modified,
                        lines_added: 0,
                        lines_deleted: 0,
                    });
                }
                Change::Rewrite {
                    source_location,
                    location,
                    ..
                } => {
                    diff_stats.push(FileDiffStats {
                        path: location.to_path_lossy().to_path_buf(),
                        old_path: Some(source_location.to_path_lossy().to_path_buf()),
                        status: DiffStatus::Renamed,
                        lines_added: 0,
                        lines_deleted: 0,
                    });
                }
            }
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        })
        .map_err(|e| HomerError::Extract(ExtractError::Git(format!("diff error: {e}"))))?;

    Ok(diff_stats)
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
}
