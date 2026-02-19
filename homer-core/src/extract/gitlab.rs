// GitLab extractor: fetches merge requests, issues, and approvals via the REST API.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, instrument, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::forge_common::{ensure_contributor, parse_issue_refs};
use super::traits::{ExtractStats, Extractor};

/// GitLab REST API extractor.
#[derive(Debug)]
pub struct GitLabExtractor {
    /// URL-encoded project path (e.g., "owner%2Frepo").
    project_path: String,
    /// Display name (e.g., "owner/repo").
    display_name: String,
    /// Base URL for the GitLab API (e.g., `https://gitlab.com/api/v4`).
    api_base: String,
    token: Option<String>,
    client: Client,
}

impl GitLabExtractor {
    /// Detect a GitLab remote from a git repo and create the extractor.
    pub fn from_repo(repo_path: &Path, config: &HomerConfig) -> Option<Self> {
        let (base_url, owner, repo) = detect_gitlab_remote(repo_path)?;
        let token_env = &config.extraction.gitlab.token_env;
        let token = std::env::var(token_env).ok();

        let display = format!("{owner}/{repo}");
        let encoded = format!("{}%2F{}", urlencod(&owner), urlencod(&repo));
        let api_base = format!("{base_url}/api/v4");

        Some(Self {
            project_path: encoded,
            display_name: display,
            api_base,
            token,
            client: Client::new(),
        })
    }

    /// Create with explicit project path (for testing).
    pub fn new(project_path: String, api_base: String, token: Option<String>) -> Self {
        let display = project_path.replace("%2F", "/");
        Self {
            project_path,
            display_name: display,
            api_base,
            token,
            client: Client::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Extractor for GitLabExtractor {
    fn name(&self) -> &'static str {
        "gitlab"
    }

    async fn has_work(&self, _store: &dyn HomerStore) -> crate::error::Result<bool> {
        Ok(self.token.is_some())
    }

    #[instrument(skip_all, name = "gitlab_extract")]
    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        info!(project = %self.display_name, "GitLab extraction starting");

        let last_mr = store
            .get_checkpoint("gitlab_last_mr")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let last_issue = store
            .get_checkpoint("gitlab_last_issue")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Fetch merge requests
        match self
            .fetch_merge_requests(store, &mut stats, last_mr, config)
            .await
        {
            Ok(max_mr) => {
                if max_mr > last_mr {
                    store
                        .set_checkpoint("gitlab_last_mr", &max_mr.to_string())
                        .await?;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to fetch merge requests");
                stats.errors.push(("merge_requests".to_string(), e));
            }
        }

        // Fetch issues
        match self.fetch_issues(store, &mut stats, last_issue).await {
            Ok(max_issue) => {
                if max_issue > last_issue {
                    store
                        .set_checkpoint("gitlab_last_issue", &max_issue.to_string())
                        .await?;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to fetch issues");
                stats.errors.push(("issues".to_string(), e));
            }
        }

        stats.duration = start.elapsed();
        info!(
            nodes = stats.nodes_created,
            edges = stats.edges_created,
            errors = stats.errors.len(),
            duration = ?stats.duration,
            "GitLab extraction complete"
        );

        Ok(stats)
    }
}

impl GitLabExtractor {
    // ── Merge Requests ──────────────────────────────────────────────

    async fn fetch_merge_requests(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_iid: u64,
        config: &HomerConfig,
    ) -> crate::error::Result<u64> {
        let mut max_iid = since_iid;
        let mut page = 1u32;

        loop {
            let mrs = self
                .api_get::<Vec<GlMergeRequest>>(&format!(
                    "/projects/{}/merge_requests?state=all&sort=asc&order_by=created_at&per_page=100&page={page}",
                    self.project_path
                ))
                .await?;

            if mrs.is_empty() {
                break;
            }

            for mr in &mrs {
                if mr.iid <= since_iid {
                    continue;
                }
                max_iid = max_iid.max(mr.iid);
                self.store_merge_request(store, stats, mr, config).await?;
            }

            if mrs.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(max_iid)
    }

    async fn store_merge_request(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        mr: &GlMergeRequest,
        config: &HomerConfig,
    ) -> crate::error::Result<()> {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), serde_json::json!(mr.title));
        metadata.insert("state".to_string(), serde_json::json!(mr.state));
        metadata.insert("iid".to_string(), serde_json::json!(mr.iid));
        metadata.insert("forge".to_string(), serde_json::json!("gitlab"));
        if let Some(desc) = &mr.description {
            metadata.insert("body".to_string(), serde_json::json!(desc));
        }
        if let Some(merged) = &mr.merged_at {
            metadata.insert("merged_at".to_string(), serde_json::json!(merged));
        }

        // Map to PullRequest node (consistent with GitHub)
        let mr_name = format!("MR!{}", mr.iid);
        let mr_node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::PullRequest,
                name: mr_name,
                content_hash: None,
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;
        stats.nodes_created += 1;

        // Authored edge
        let contrib_id = ensure_contributor(store, stats, &mr.author.username).await?;
        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Authored,
                members: vec![
                    HyperedgeMember {
                        node_id: contrib_id,
                        role: "author".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: mr_node_id,
                        role: "artifact".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.edges_created += 1;

        // Approvals → Reviewed edges
        if config.extraction.gitlab.include_reviews {
            self.fetch_approvals(store, stats, mr.iid, mr_node_id)
                .await?;
        }

        // Parse issue cross-references from description
        if let Some(desc) = &mr.description {
            let refs = parse_issue_refs(desc);
            for issue_iid in refs {
                let issue_name = format!("GLIssue#{issue_iid}");
                let issue_id = match store.get_node_by_name(NodeKind::Issue, &issue_name).await? {
                    Some(n) => n.id,
                    None => {
                        store
                            .upsert_node(&Node {
                                id: NodeId(0),
                                kind: NodeKind::Issue,
                                name: issue_name,
                                content_hash: None,
                                last_extracted: Utc::now(),
                                metadata: HashMap::new(),
                            })
                            .await?
                    }
                };

                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Resolves,
                        members: vec![
                            HyperedgeMember {
                                node_id: mr_node_id,
                                role: "resolver".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: issue_id,
                                role: "resolved".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 0.9,
                        last_updated: Utc::now(),
                        metadata: HashMap::new(),
                    })
                    .await?;
                stats.edges_created += 1;
            }
        }

        Ok(())
    }

    async fn fetch_approvals(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        mr_iid: u64,
        mr_node_id: NodeId,
    ) -> crate::error::Result<()> {
        let approvals = self
            .api_get::<GlApprovals>(&format!(
                "/projects/{}/merge_requests/{mr_iid}/approvals",
                self.project_path
            ))
            .await;

        let approvals = match approvals {
            Ok(a) => a,
            Err(e) => {
                debug!(mr_iid, error = %e, "Failed to fetch approvals");
                return Ok(());
            }
        };

        for approver in &approvals.approved_by {
            let reviewer_id = ensure_contributor(store, stats, &approver.user.username).await?;

            let mut meta = HashMap::new();
            meta.insert("forge".to_string(), serde_json::json!("gitlab"));
            meta.insert("verdict".to_string(), serde_json::json!("approved"));

            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Reviewed,
                    members: vec![
                        HyperedgeMember {
                            node_id: reviewer_id,
                            role: "reviewer".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: mr_node_id,
                            role: "artifact".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: Utc::now(),
                    metadata: meta,
                })
                .await?;
            stats.edges_created += 1;
        }

        Ok(())
    }

    // ── Issues ──────────────────────────────────────────────────────

    async fn fetch_issues(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_iid: u64,
    ) -> crate::error::Result<u64> {
        let mut max_iid = since_iid;
        let mut page = 1u32;

        loop {
            let issues = self
                .api_get::<Vec<GlIssue>>(&format!(
                    "/projects/{}/issues?state=all&sort=asc&order_by=created_at&per_page=100&page={page}",
                    self.project_path
                ))
                .await?;

            if issues.is_empty() {
                break;
            }

            for issue in &issues {
                if issue.iid <= since_iid {
                    continue;
                }
                max_iid = max_iid.max(issue.iid);

                let mut metadata = HashMap::new();
                metadata.insert("title".to_string(), serde_json::json!(issue.title));
                metadata.insert("state".to_string(), serde_json::json!(issue.state));
                metadata.insert("iid".to_string(), serde_json::json!(issue.iid));
                metadata.insert("forge".to_string(), serde_json::json!("gitlab"));
                if let Some(desc) = &issue.description {
                    metadata.insert("body".to_string(), serde_json::json!(desc));
                }
                let labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
                metadata.insert("labels".to_string(), serde_json::json!(labels));

                let issue_name = format!("GLIssue#{}", issue.iid);
                store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::Issue,
                        name: issue_name,
                        content_hash: None,
                        last_extracted: Utc::now(),
                        metadata,
                    })
                    .await?;
                stats.nodes_created += 1;
            }

            if issues.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(max_iid)
    }

    // ── HTTP Client ─────────────────────────────────────────────────

    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> crate::error::Result<T> {
        let url = format!("{}{path}", self.api_base);

        let mut req = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("User-Agent", "homer-cli/0.1");

        if let Some(token) = &self.token {
            req = req.header("PRIVATE-TOKEN", token.as_str());
        }

        debug!(url = %url, "GitLab API request");

        let resp = req
            .send()
            .await
            .map_err(|e| HomerError::Extract(ExtractError::Git(format!("GitLab API: {e}"))))?;

        // Check rate limit headers
        if let Some(remaining) = resp
            .headers()
            .get("ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
        {
            if remaining < 10 {
                warn!(remaining, "GitLab API rate limit low");
            }
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(HomerError::Extract(ExtractError::Git(format!(
                "GitLab API {status}: {body}"
            ))));
        }

        resp.json()
            .await
            .map_err(|e| HomerError::Extract(ExtractError::Git(format!("Parse response: {e}"))))
    }
}

// ── GitLab API Types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GlMergeRequest {
    iid: u64,
    title: String,
    state: String,
    description: Option<String>,
    merged_at: Option<String>,
    author: GlUser,
}

#[derive(Debug, Deserialize)]
struct GlIssue {
    iid: u64,
    title: String,
    state: String,
    description: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GlUser {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlApprovals {
    #[serde(default)]
    approved_by: Vec<GlApprover>,
}

#[derive(Debug, Deserialize)]
struct GlApprover {
    user: GlUser,
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Minimal percent-encoding for GitLab project path segments.
fn urlencod(s: &str) -> String {
    s.replace('%', "%25").replace('/', "%2F")
}

/// Detect GitLab remote URL from a git repo.
/// Returns (`base_url`, owner, repo) if a GitLab remote is found.
fn detect_gitlab_remote(repo_path: &Path) -> Option<(String, String, String)> {
    let repo = gix::open(repo_path).ok()?;
    let remote = repo
        .find_default_remote(gix::remote::Direction::Push)?
        .ok()?;
    let url = remote.url(gix::remote::Direction::Push)?;
    let url_str = url.to_bstring().to_string();

    parse_gitlab_url(&url_str)
}

/// Parse owner/repo and base URL from a GitLab URL (SSH or HTTPS).
/// Supports gitlab.com and self-hosted instances.
fn parse_gitlab_url(url: &str) -> Option<(String, String, String)> {
    // SSH: git@gitlab.example.com:owner/repo.git
    if url.starts_with("git@") {
        let rest = url.strip_prefix("git@")?;
        let (host, path) = rest.split_once(':')?;
        if !host.contains("gitlab") {
            return None;
        }
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, repo) = path.split_once('/')?;
        let base = format!("https://{host}");
        return Some((base, owner.to_string(), repo.to_string()));
    }

    // HTTPS: https://gitlab.example.com/owner/repo.git
    if let Some(pos) = url.find("gitlab") {
        // Find the host portion
        let scheme_end = url.find("://").map_or(0, |p| p + 3);
        let host_and_path = &url[scheme_end..];
        let (host, path) = host_and_path.split_once('/')?;

        if !host.contains("gitlab") {
            return None;
        }

        let _ = pos; // used only for the initial check
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, repo) = path.split_once('/')?;
        // Handle subgroups: take only the last segment as repo
        let base = format!("https://{host}");
        return Some((base, owner.to_string(), repo.to_string()));
    }

    None
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_gitlab_url() {
        let (base, owner, repo) = parse_gitlab_url("git@gitlab.com:myorg/myrepo.git").unwrap();
        assert_eq!(base, "https://gitlab.com");
        assert_eq!(owner, "myorg");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_https_gitlab_url() {
        let (base, owner, repo) = parse_gitlab_url("https://gitlab.com/myorg/myrepo.git").unwrap();
        assert_eq!(base, "https://gitlab.com");
        assert_eq!(owner, "myorg");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_https_no_git_suffix() {
        let (_, owner, repo) = parse_gitlab_url("https://gitlab.com/foo/bar").unwrap();
        assert_eq!(owner, "foo");
        assert_eq!(repo, "bar");
    }

    #[test]
    fn parse_self_hosted_gitlab() {
        let (base, owner, repo) =
            parse_gitlab_url("git@gitlab.internal.corp:team/service.git").unwrap();
        assert_eq!(base, "https://gitlab.internal.corp");
        assert_eq!(owner, "team");
        assert_eq!(repo, "service");
    }

    #[test]
    fn non_gitlab_url_returns_none() {
        assert!(parse_gitlab_url("https://github.com/foo/bar").is_none());
        assert!(parse_gitlab_url("git@github.com:foo/bar.git").is_none());
    }

    #[test]
    fn urlencod_simple() {
        assert_eq!(urlencod("myorg"), "myorg");
        assert_eq!(urlencod("my/org"), "my%2Forg");
    }
}
