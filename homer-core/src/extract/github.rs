// GitHub extractor: fetches PRs, issues, and reviews via the REST API.
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
use tracing::{debug, info, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::traits::ExtractStats;

/// GitHub REST API extractor.
#[derive(Debug)]
pub struct GitHubExtractor {
    owner: String,
    repo: String,
    token: Option<String>,
    client: Client,
}

impl GitHubExtractor {
    /// Detect GitHub remote from a git repo and create the extractor.
    pub fn from_repo(repo_path: &Path, _config: &HomerConfig) -> Option<Self> {
        let remote_url = detect_github_remote(repo_path)?;
        let (owner, repo) = parse_github_url(&remote_url)?;
        let token = std::env::var("GITHUB_TOKEN").ok();

        Some(Self {
            owner,
            repo,
            token,
            client: Client::new(),
        })
    }

    /// Create with explicit owner/repo (for testing).
    pub fn new(owner: String, repo: String, token: Option<String>) -> Self {
        Self {
            owner,
            repo,
            token,
            client: Client::new(),
        }
    }

    pub async fn extract(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        info!(owner = %self.owner, repo = %self.repo, "GitHub extraction starting");

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

        // Fetch PRs
        match self.fetch_pull_requests(store, &mut stats, last_pr).await {
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
        match self.fetch_issues(store, &mut stats, last_issue).await {
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

    // ── Pull Requests ───────────────────────────────────────────────

    async fn fetch_pull_requests(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_number: u64,
    ) -> crate::error::Result<u64> {
        let mut max_number = since_number;
        let mut page = 1u32;

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
                max_number = max_number.max(pr.number);

                self.store_pull_request(store, stats, pr).await?;
            }

            if prs.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(max_number)
    }

    async fn store_pull_request(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        pr: &GhPullRequest,
    ) -> crate::error::Result<()> {
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
                let issue_id = match store
                    .get_node_by_name(NodeKind::Issue, &issue_name)
                    .await?
                {
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

        Ok(())
    }

    // ── Issues ──────────────────────────────────────────────────────

    async fn fetch_issues(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        since_number: u64,
    ) -> crate::error::Result<u64> {
        let mut max_number = since_number;
        let mut page = 1u32;

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
                max_number = max_number.max(issue.number);

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

            if issues.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(max_number)
    }

    // ── HTTP Client ─────────────────────────────────────────────────

    async fn api_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> crate::error::Result<T> {
        let url = format!("https://api.github.com{path}");

        let mut req = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "homer-cli/0.1");

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        debug!(url = %url, "GitHub API request");

        let resp = req
            .send()
            .await
            .map_err(|e| HomerError::Extract(ExtractError::Git(format!("GitHub API: {e}"))))?;

        // Check rate limit headers
        if let Some(remaining) = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
        {
            if remaining < 10 {
                warn!(remaining, "GitHub API rate limit low");
            }
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(HomerError::Extract(ExtractError::Git(format!(
                "GitHub API {status}: {body}"
            ))));
        }

        resp.json()
            .await
            .map_err(|e| HomerError::Extract(ExtractError::Git(format!("Parse response: {e}"))))
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

// ── Helpers ─────────────────────────────────────────────────────────

/// Ensure a Contributor node exists, return its ID.
async fn ensure_contributor(
    store: &dyn HomerStore,
    stats: &mut ExtractStats,
    login: &str,
) -> crate::error::Result<NodeId> {
    if let Some(node) = store
        .get_node_by_name(NodeKind::Contributor, login)
        .await?
    {
        return Ok(node.id);
    }

    let id = store
        .upsert_node(&Node {
            id: NodeId(0),
            kind: NodeKind::Contributor,
            name: login.to_string(),
            content_hash: None,
            last_extracted: Utc::now(),
            metadata: HashMap::new(),
        })
        .await?;
    stats.nodes_created += 1;
    Ok(id)
}

/// Parse issue cross-references from PR body text.
/// Matches patterns like "fixes #123", "closes #456", "resolves #789".
fn parse_issue_refs(text: &str) -> Vec<u64> {
    let lower = text.to_lowercase();
    let mut refs = Vec::new();

    let patterns = [
        "close ", "closes ", "closed ",
        "fix ", "fixes ", "fixed ",
        "resolve ", "resolves ", "resolved ",
    ];

    for pattern in &patterns {
        let mut search = lower.as_str();
        while let Some(pos) = search.find(pattern) {
            let after = &search[pos + pattern.len()..];
            if let Some(num) = extract_issue_number(after) {
                if !refs.contains(&num) {
                    refs.push(num);
                }
            }
            search = &search[pos + pattern.len()..];
        }
    }

    refs
}

/// Extract an issue number after a keyword, e.g., "#123" or "org/repo#123".
fn extract_issue_number(text: &str) -> Option<u64> {
    let text = text.trim_start();
    let text = if let Some(rest) = text.strip_prefix('#') {
        rest
    } else {
        // Could be "org/repo#123" — skip to #
        let (_, after) = text.split_once('#')?;
        after
    };

    let num_str: String = text.chars().take_while(char::is_ascii_digit).collect();
    if num_str.is_empty() {
        return None;
    }
    num_str.parse().ok()
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
    fn parse_issue_refs_basic() {
        let refs = parse_issue_refs("This fixes #42 and closes #99");
        assert!(refs.contains(&42));
        assert!(refs.contains(&99));
    }

    #[test]
    fn parse_issue_refs_case_insensitive() {
        let refs = parse_issue_refs("FIXES #10, Resolves #20");
        assert!(refs.contains(&10));
        assert!(refs.contains(&20));
    }

    #[test]
    fn parse_issue_refs_no_duplicates() {
        let refs = parse_issue_refs("fixes #5, also fixes #5");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn parse_issue_refs_org_repo_syntax() {
        let refs = parse_issue_refs("fixes org/repo#123");
        assert!(refs.contains(&123));
    }

    #[test]
    fn parse_issue_refs_no_refs() {
        let refs = parse_issue_refs("This PR adds a feature");
        assert!(refs.is_empty());
    }

    #[test]
    fn extract_number_from_hash() {
        assert_eq!(extract_issue_number("#42"), Some(42));
        assert_eq!(extract_issue_number("  #100"), Some(100));
        assert_eq!(extract_issue_number("#abc"), None);
    }
}
