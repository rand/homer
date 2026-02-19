// GitHub extractor: fetches PRs, issues, and reviews via the REST API.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Maximum retry attempts for rate-limited requests.
const MAX_RETRIES: u32 = 5;
/// Pause and wait for reset when remaining drops below this threshold.
const RATE_LIMIT_PAUSE_THRESHOLD: u32 = 5;

/// GitHub REST API extractor.
#[derive(Debug)]
pub struct GitHubExtractor {
    owner: String,
    repo: String,
    token: Option<String>,
    client: Client,
    /// Remaining API calls before rate limit resets.
    rate_remaining: Cell<u32>,
    /// Unix timestamp when the rate limit window resets.
    rate_reset: Cell<u64>,
}

impl GitHubExtractor {
    /// Detect GitHub remote from a git repo and create the extractor.
    pub fn from_repo(repo_path: &Path, config: &HomerConfig) -> Option<Self> {
        let remote_url = detect_github_remote(repo_path)?;
        let (owner, repo) = parse_github_url(&remote_url)?;
        let token_env = &config.extraction.github.token_env;
        let token = std::env::var(token_env).ok();

        Some(Self {
            owner,
            repo,
            token,
            client: Client::new(),
            rate_remaining: Cell::new(u32::MAX),
            rate_reset: Cell::new(0),
        })
    }

    /// Create with explicit owner/repo (for testing).
    pub fn new(owner: String, repo: String, token: Option<String>) -> Self {
        Self {
            owner,
            repo,
            token,
            client: Client::new(),
            rate_remaining: Cell::new(u32::MAX),
            rate_reset: Cell::new(0),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Extractor for GitHubExtractor {
    fn name(&self) -> &'static str {
        "github"
    }

    async fn has_work(&self, _store: &dyn HomerStore) -> crate::error::Result<bool> {
        Ok(self.token.is_some())
    }

    #[instrument(skip_all, name = "github_extract")]
    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();
        let gh_config = &config.extraction.github;

        let estimated = estimate_api_calls(gh_config);
        info!(
            owner = %self.owner,
            repo = %self.repo,
            estimated_api_calls = estimated,
            "GitHub extraction starting"
        );

        // Get checkpoints for incremental updates
        let last_pr = store
            .get_checkpoint("github_last_pr")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let last_issue = store
            .get_checkpoint("github_last_issue")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Fetch PRs (with optional reviews and comments)
        match self
            .fetch_pull_requests(store, &mut stats, last_pr, gh_config)
            .await
        {
            Ok(max_pr) => {
                if max_pr > last_pr {
                    store
                        .set_checkpoint("github_last_pr", &max_pr.to_string())
                        .await?;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to fetch pull requests");
                stats.errors.push(("pull_requests".to_string(), e));
            }
        }

        // Fetch Issues
        match self
            .fetch_issues(store, &mut stats, last_issue, gh_config)
            .await
        {
            Ok(max_issue) => {
                if max_issue > last_issue {
                    store
                        .set_checkpoint("github_last_issue", &max_issue.to_string())
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
            "GitHub extraction complete"
        );

        Ok(stats)
    }
}

impl GitHubExtractor {
    // ── Pull Requests ───────────────────────────────────────────────

    async fn fetch_pull_requests(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_number: u64,
        gh_config: &crate::config::GitHubExtractionConfig,
    ) -> crate::error::Result<u64> {
        let mut max_number = since_number;
        let mut page = 1u32;
        let mut fetched = 0u32;

        loop {
            let prs = self
                .api_get::<Vec<GhPullRequest>>(&format!(
                    "/repos/{}/{}/pulls?state=all&sort=created&direction=asc&per_page=100&page={page}",
                    self.owner, self.repo
                ))
                .await?;

            if prs.is_empty() {
                break;
            }

            for pr in &prs {
                if pr.number <= since_number {
                    continue;
                }
                if gh_config.max_pr_history > 0 && fetched >= gh_config.max_pr_history {
                    break;
                }
                max_number = max_number.max(pr.number);
                fetched += 1;

                let pr_node_id = self.store_pull_request(store, stats, pr).await?;

                if gh_config.include_reviews {
                    if let Err(e) = self
                        .fetch_pr_reviews(store, stats, pr.number, pr_node_id)
                        .await
                    {
                        debug!(pr = pr.number, error = %e, "Failed to fetch reviews");
                    }
                }

                if gh_config.include_comments {
                    if let Err(e) = self.fetch_pr_comments(store, pr.number, pr_node_id).await {
                        debug!(pr = pr.number, error = %e, "Failed to fetch comments");
                    }
                }
            }

            if prs.len() < 100
                || (gh_config.max_pr_history > 0 && fetched >= gh_config.max_pr_history)
            {
                break;
            }
            page += 1;
        }

        Ok(max_number)
    }

    #[allow(clippy::too_many_lines)]
    async fn store_pull_request(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        pr: &GhPullRequest,
    ) -> crate::error::Result<NodeId> {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), serde_json::json!(pr.title));
        metadata.insert("state".to_string(), serde_json::json!(pr.state));
        metadata.insert("number".to_string(), serde_json::json!(pr.number));
        if let Some(body) = &pr.body {
            metadata.insert("body".to_string(), serde_json::json!(body));
        }
        if let Some(merged) = &pr.merged_at {
            metadata.insert("merged_at".to_string(), serde_json::json!(merged));
        }
        if let Some(sha) = &pr.merge_commit_sha {
            metadata.insert("merge_commit_sha".to_string(), serde_json::json!(sha));
        }
        if let Some(user) = &pr.user {
            metadata.insert("author".to_string(), serde_json::json!(user.login));
        }

        let pr_name = format!("PR#{}", pr.number);
        let pr_node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::PullRequest,
                name: pr_name,
                content_hash: None,
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;
        stats.nodes_created += 1;

        // Create Authored edge from contributor
        if let Some(user) = &pr.user {
            let contrib_id = ensure_contributor(store, stats, &user.login).await?;
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
                            node_id: pr_node_id,
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
        }

        // Parse cross-references for Resolves edges
        if let Some(body) = &pr.body {
            let refs = parse_issue_refs(body);
            for issue_num in refs {
                let issue_name = format!("Issue#{issue_num}");
                // Get or create Issue node
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
                                node_id: pr_node_id,
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

        // Link PR to its merge commit if available
        if let Some(sha) = &pr.merge_commit_sha {
            if let Some(commit_node) = store.get_node_by_name(NodeKind::Commit, sha).await? {
                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Includes,
                        members: vec![
                            HyperedgeMember {
                                node_id: pr_node_id,
                                role: "pull_request".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: commit_node.id,
                                role: "merge_commit".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 1.0,
                        last_updated: Utc::now(),
                        metadata: HashMap::new(),
                    })
                    .await?;
                stats.edges_created += 1;
            }
        }

        Ok(pr_node_id)
    }

    // ── Reviews & Comments ───────────────────────────────────────

    async fn fetch_pr_reviews(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        pr_number: u64,
        pr_node_id: NodeId,
    ) -> crate::error::Result<()> {
        let reviews = self
            .api_get::<Vec<GhReview>>(&format!(
                "/repos/{}/{}/pulls/{pr_number}/reviews",
                self.owner, self.repo
            ))
            .await?;

        for review in &reviews {
            let Some(user) = &review.user else {
                continue;
            };

            let reviewer_id = ensure_contributor(store, stats, &user.login).await?;

            let mut edge_meta = HashMap::new();
            edge_meta.insert("state".to_string(), serde_json::json!(review.state));
            if let Some(submitted) = &review.submitted_at {
                edge_meta.insert("submitted_at".to_string(), serde_json::json!(submitted));
            }
            if let Some(body) = &review.body {
                if !body.is_empty() {
                    edge_meta.insert("body".to_string(), serde_json::json!(body));
                }
            }

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
                            node_id: pr_node_id,
                            role: "artifact".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: Utc::now(),
                    metadata: edge_meta,
                })
                .await?;
            stats.edges_created += 1;
        }

        Ok(())
    }

    async fn fetch_pr_comments(
        &self,
        store: &dyn HomerStore,
        pr_number: u64,
        pr_node_id: NodeId,
    ) -> crate::error::Result<()> {
        let comments = self
            .api_get::<Vec<GhComment>>(&format!(
                "/repos/{}/{}/issues/{pr_number}/comments",
                self.owner, self.repo
            ))
            .await?;

        if comments.is_empty() {
            return Ok(());
        }

        // Store comments as metadata on the PR node
        let comment_data: Vec<serde_json::Value> = comments
            .iter()
            .map(|c| {
                serde_json::json!({
                    "author": c.user.as_ref().map_or("unknown", |u| u.login.as_str()),
                    "body": c.body,
                    "created_at": c.created_at,
                })
            })
            .collect();

        // Fetch current node, add comments to metadata, re-upsert
        if let Some(mut node) = store.get_node(pr_node_id).await? {
            node.metadata
                .insert("comments".to_string(), serde_json::json!(comment_data));
            node.metadata.insert(
                "comment_count".to_string(),
                serde_json::json!(comment_data.len()),
            );
            store.upsert_node(&node).await?;
        }

        Ok(())
    }

    // ── Issues ──────────────────────────────────────────────────────

    async fn fetch_issues(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_number: u64,
        gh_config: &crate::config::GitHubExtractionConfig,
    ) -> crate::error::Result<u64> {
        let mut max_number = since_number;
        let mut page = 1u32;
        let mut fetched = 0u32;

        loop {
            let issues = self
                .api_get::<Vec<GhIssue>>(&format!(
                    "/repos/{}/{}/issues?state=all&sort=created&direction=asc&per_page=100&page={page}&filter=all",
                    self.owner, self.repo
                ))
                .await?;

            if issues.is_empty() {
                break;
            }

            for issue in &issues {
                // GitHub issues API includes PRs; skip those
                if issue.pull_request.is_some() {
                    continue;
                }
                if issue.number <= since_number {
                    continue;
                }
                if gh_config.max_issue_history > 0 && fetched >= gh_config.max_issue_history {
                    break;
                }
                max_number = max_number.max(issue.number);
                fetched += 1;

                let mut metadata = HashMap::new();
                metadata.insert("title".to_string(), serde_json::json!(issue.title));
                metadata.insert("state".to_string(), serde_json::json!(issue.state));
                metadata.insert("number".to_string(), serde_json::json!(issue.number));
                if let Some(body) = &issue.body {
                    metadata.insert("body".to_string(), serde_json::json!(body));
                }

                let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
                metadata.insert("labels".to_string(), serde_json::json!(labels));

                let issue_name = format!("Issue#{}", issue.number);
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

            if issues.len() < 100
                || (gh_config.max_issue_history > 0 && fetched >= gh_config.max_issue_history)
            {
                break;
            }
            page += 1;
        }

        Ok(max_number)
    }

    // ── HTTP Client ─────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> crate::error::Result<T> {
        let url = format!("https://api.github.com{path}");

        // Pre-check: if remaining is low, wait for reset
        self.wait_for_rate_reset().await;

        let mut delay = Duration::from_secs(1);

        for attempt in 0..=MAX_RETRIES {
            let mut req = self
                .client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "homer-cli/0.1");

            if let Some(token) = &self.token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }

            debug!(url = %url, attempt, "GitHub API request");

            let resp = req
                .send()
                .await
                .map_err(|e| HomerError::Extract(ExtractError::Git(format!("GitHub API: {e}"))))?;

            self.update_rate_limit(&resp);

            if resp.status().is_success() {
                return resp.json().await.map_err(|e| {
                    HomerError::Extract(ExtractError::Git(format!("Parse response: {e}")))
                });
            }

            // Rate limited — retry with backoff
            let status = resp.status().as_u16();
            if (status == 403 || status == 429) && attempt < MAX_RETRIES {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .map_or(delay, Duration::from_secs);
                warn!(
                    attempt,
                    status,
                    wait_secs = wait.as_secs(),
                    "Rate limited, backing off"
                );
                tokio::time::sleep(wait).await;
                delay = (delay * 2).min(Duration::from_secs(60));
                continue;
            }

            let body = resp.text().await.unwrap_or_default();
            return Err(HomerError::Extract(ExtractError::Git(format!(
                "GitHub API {status}: {body}"
            ))));
        }

        Err(HomerError::Extract(ExtractError::Git(format!(
            "GitHub API: max retries exceeded for {url}"
        ))))
    }

    /// Update rate limit state from response headers.
    fn update_rate_limit(&self, resp: &reqwest::Response) {
        if let Some(remaining) = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
        {
            self.rate_remaining.set(remaining);
            if remaining < 10 {
                warn!(remaining, "GitHub API rate limit low");
            }
        }
        if let Some(reset) = resp
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            self.rate_reset.set(reset);
        }
    }

    /// Sleep until the rate limit window resets if remaining is low.
    async fn wait_for_rate_reset(&self) {
        if self.rate_remaining.get() > RATE_LIMIT_PAUSE_THRESHOLD {
            return;
        }
        let reset_at = self.rate_reset.get();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if reset_at > now {
            let wait = reset_at - now + 1;
            warn!(
                remaining = self.rate_remaining.get(),
                wait_secs = wait,
                "Rate limit low, waiting for reset"
            );
            tokio::time::sleep(Duration::from_secs(wait)).await;
        }
    }
}

// ── GitHub API Types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GhPullRequest {
    number: u64,
    title: String,
    state: String,
    body: Option<String>,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
    user: Option<GhUser>,
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    state: String,
    body: Option<String>,
    labels: Vec<GhLabel>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    user: Option<GhUser>,
    state: String,
    body: Option<String>,
    submitted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhComment {
    user: Option<GhUser>,
    body: String,
    created_at: String,
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Estimate upper bound of API calls for a GitHub extraction run.
/// When limits are 0 (unlimited), uses a reasonable upper bound for estimation.
fn estimate_api_calls(gh_config: &crate::config::GitHubExtractionConfig) -> u32 {
    let pr_limit = if gh_config.max_pr_history == 0 {
        1000
    } else {
        gh_config.max_pr_history
    };
    let issue_limit = if gh_config.max_issue_history == 0 {
        1000
    } else {
        gh_config.max_issue_history
    };
    let pr_pages = pr_limit / 100 + 1;
    let mut calls = pr_pages;
    if gh_config.include_reviews {
        calls += pr_limit;
    }
    if gh_config.include_comments {
        calls += pr_limit;
    }
    calls += issue_limit / 100 + 1;
    calls
}

/// Detect GitHub remote URL from a git repo.
fn detect_github_remote(repo_path: &Path) -> Option<String> {
    let repo = gix::open(repo_path).ok()?;
    let remote = repo
        .find_default_remote(gix::remote::Direction::Push)?
        .ok()?;
    let url = remote.url(gix::remote::Direction::Push)?;
    let url_str = url.to_bstring().to_string();
    if url_str.contains("github.com") {
        Some(url_str)
    } else {
        None
    }
}

/// Parse "owner/repo" from a GitHub URL (SSH or HTTPS).
fn parse_github_url(url: &str) -> Option<(String, String)> {
    // SSH: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let (owner, repo) = rest.split_once('/')?;
        return Some((owner.to_string(), repo.to_string()));
    }

    // HTTPS: https://github.com/owner/repo.git
    if let Some((_, after)) = url.split_once("github.com/") {
        let after = after.strip_suffix(".git").unwrap_or(after);
        let (owner, repo) = after.split_once('/')?;
        return Some((owner.to_string(), repo.to_string()));
    }

    None
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_github_url() {
        let (owner, repo) = parse_github_url("git@github.com:rand/homer.git").unwrap();
        assert_eq!(owner, "rand");
        assert_eq!(repo, "homer");
    }

    #[test]
    fn parse_https_github_url() {
        let (owner, repo) =
            parse_github_url("https://github.com/anthropics/claude-code.git").unwrap();
        assert_eq!(owner, "anthropics");
        assert_eq!(repo, "claude-code");
    }

    #[test]
    fn parse_https_no_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/foo/bar").unwrap();
        assert_eq!(owner, "foo");
        assert_eq!(repo, "bar");
    }

    #[test]
    fn non_github_url_returns_none() {
        assert!(parse_github_url("https://gitlab.com/foo/bar").is_none());
    }

    #[test]
    fn estimate_api_calls_upper_bound() {
        let config = crate::config::GitHubExtractionConfig {
            max_pr_history: 100,
            max_issue_history: 200,
            include_reviews: true,
            include_comments: true,
            ..crate::config::GitHubExtractionConfig::default()
        };
        // PR pages (2) + reviews (100) + comments (100) + issue pages (3) = 205
        let est = estimate_api_calls(&config);
        assert_eq!(est, 205);
    }

    #[test]
    fn estimate_api_calls_no_extras() {
        let config = crate::config::GitHubExtractionConfig {
            max_pr_history: 100,
            max_issue_history: 200,
            include_reviews: false,
            include_comments: false,
            ..crate::config::GitHubExtractionConfig::default()
        };
        // PR pages (2) + issue pages (3) = 5
        let est = estimate_api_calls(&config);
        assert_eq!(est, 5);
    }

    #[test]
    fn rate_limit_fields_initialized() {
        let ext = GitHubExtractor::new("owner".to_string(), "repo".to_string(), None);
        assert_eq!(ext.rate_remaining.get(), u32::MAX);
        assert_eq!(ext.rate_reset.get(), 0);
    }

    #[test]
    fn github_config_defaults() {
        let config = crate::config::GitHubExtractionConfig::default();
        assert_eq!(config.token_env, "GITHUB_TOKEN");
        assert_eq!(config.max_pr_history, 500);
        assert_eq!(config.max_issue_history, 1000);
        assert!(config.include_comments);
        assert!(config.include_reviews);
    }

    #[test]
    fn deserialize_review() {
        let json = r#"{
            "user": {"login": "reviewer1"},
            "state": "APPROVED",
            "body": "Looks good!",
            "submitted_at": "2024-01-01T00:00:00Z"
        }"#;
        let review: GhReview = serde_json::from_str(json).unwrap();
        assert_eq!(review.state, "APPROVED");
        assert_eq!(review.user.unwrap().login, "reviewer1");
        assert_eq!(review.body.unwrap(), "Looks good!");
    }

    #[test]
    fn deserialize_comment() {
        let json = r#"{
            "user": {"login": "commenter"},
            "body": "Nice work",
            "created_at": "2024-01-02T12:00:00Z"
        }"#;
        let comment: GhComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.body, "Nice work");
        assert_eq!(comment.created_at, "2024-01-02T12:00:00Z");
    }

    #[test]
    fn deserialize_review_without_body() {
        let json = r#"{
            "user": {"login": "reviewer2"},
            "state": "COMMENTED",
            "body": null,
            "submitted_at": null
        }"#;
        let review: GhReview = serde_json::from_str(json).unwrap();
        assert_eq!(review.state, "COMMENTED");
        assert!(review.body.is_none());
        assert!(review.submitted_at.is_none());
    }
}
