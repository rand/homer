#![allow(clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};
use tracing::{info, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::traits::ExtractStats;

// ── Public extractor ─────────────────────────────────────────────

#[derive(Debug)]
pub struct PromptExtractor {
    repo_path: PathBuf,
}

impl PromptExtractor {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }

    pub async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        // Agent rule files are ALWAYS extracted (they're committed to the repo).
        self.extract_agent_rules(store, &mut stats).await;

        // Prompt session extraction is opt-in.
        if !config.extraction.prompts.enabled {
            info!("Prompt session extraction disabled (opt-in via config)");
            stats.duration = start.elapsed();
            return Ok(stats);
        }

        let sources = &config.extraction.prompts.sources;

        if sources.iter().any(|s| s == "claude-code") {
            self.extract_claude_code_sessions(store, config, &mut stats)
                .await;
        }

        // Correlate sessions with commits by shared modified files
        self.correlate_sessions_with_commits(store, &mut stats)
            .await;

        stats.duration = start.elapsed();
        info!(
            nodes = stats.nodes_created,
            edges = stats.edges_created,
            errors = stats.errors.len(),
            duration = ?stats.duration,
            "Prompt extraction complete"
        );
        Ok(stats)
    }
}

// ── Agent rule extraction (always runs) ──────────────────────────

/// Known agent rule file locations.
const AGENT_RULE_GLOBS: &[&str] = &[
    "CLAUDE.md",
    ".claude/settings.json",
    ".cursor/rules/*.mdc",
    ".cursor/rules/*.md",
    ".windsurf/rules/*.md",
    ".clinerules/*.md",
    "AGENTS.md",
];

impl PromptExtractor {
    async fn extract_agent_rules(&self, store: &dyn HomerStore, stats: &mut ExtractStats) {
        let rule_files = self.find_agent_rule_files();
        if rule_files.is_empty() {
            return;
        }

        info!(count = rule_files.len(), "Found agent rule files");

        for path in &rule_files {
            match self.process_agent_rule(store, path).await {
                Ok((nodes, edges)) => {
                    stats.nodes_created += nodes;
                    stats.edges_created += edges;
                }
                Err(e) => {
                    let p = path.to_string_lossy().to_string();
                    warn!(path = %p, error = %e, "Failed to process agent rule");
                    stats.errors.push((p, e));
                }
            }
        }
    }

    fn find_agent_rule_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for pattern in AGENT_RULE_GLOBS {
            let full = self.repo_path.join(pattern).to_string_lossy().to_string();
            if let Ok(paths) = glob::glob(&full) {
                for entry in paths.flatten() {
                    if entry.is_file() {
                        found.push(entry);
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        found
    }

    async fn process_agent_rule(
        &self,
        store: &dyn HomerStore,
        path: &Path,
    ) -> crate::error::Result<(u64, u64)> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;

        let relative = path.strip_prefix(&self.repo_path).unwrap_or(path);
        let content_hash = hash_str(&content);

        // Check if content changed since last extraction.
        if let Ok(Some(existing)) = store
            .get_node_by_name(NodeKind::AgentRule, &relative.to_string_lossy())
            .await
        {
            if existing.content_hash == Some(content_hash) {
                return Ok((0, 0));
            }
        }

        let mut metadata = HashMap::new();
        let source = classify_rule_source(relative);
        metadata.insert("source".to_string(), serde_json::json!(source));
        metadata.insert("size_bytes".to_string(), serde_json::json!(content.len()));

        // Extract referenced file paths from the rule content.
        let refs = extract_file_references(&content);
        metadata.insert("referenced_files".to_string(), serde_json::json!(refs));

        let rule_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::AgentRule,
                name: relative.to_string_lossy().to_string(),
                content_hash: Some(content_hash),
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;

        let mut edges = 0u64;

        // Create PromptReferences edges to any File nodes mentioned.
        for file_ref in &refs {
            if let Ok(Some(file_node)) = store.get_node_by_name(NodeKind::File, file_ref).await {
                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::PromptReferences,
                        members: vec![
                            HyperedgeMember {
                                node_id: rule_id,
                                role: "rule".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: file_node.id,
                                role: "file".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 0.8,
                        last_updated: Utc::now(),
                        metadata: HashMap::new(),
                    })
                    .await?;
                edges += 1;
            }
        }

        Ok((1, edges))
    }
}

// ── Claude Code session extraction ───────────────────────────────

/// Represents a single parsed interaction from a Claude Code session.
#[derive(Debug)]
struct AgentInteraction {
    session_id: String,
    referenced_files: Vec<String>,
    modified_files: Vec<String>,
    timestamp: DateTime<Utc>,
    had_correction: bool,
    tool_uses: u32,
}

impl PromptExtractor {
    async fn extract_claude_code_sessions(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        stats: &mut ExtractStats,
    ) {
        let claude_dir = self.repo_path.join(".claude");
        if !claude_dir.is_dir() {
            return;
        }

        let session_files = find_session_files(&claude_dir);
        if session_files.is_empty() {
            return;
        }

        info!(
            count = session_files.len(),
            "Found Claude Code session files"
        );

        for path in &session_files {
            match self.process_session_file(store, config, path).await {
                Ok((nodes, edges)) => {
                    stats.nodes_created += nodes;
                    stats.edges_created += edges;
                }
                Err(e) => {
                    let p = path.to_string_lossy().to_string();
                    warn!(path = %p, error = %e, "Failed to process session file");
                    stats.errors.push((p, e));
                }
            }
        }
    }

    async fn process_session_file(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        path: &Path,
    ) -> crate::error::Result<(u64, u64)> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;

        let interactions = parse_claude_code_jsonl(&content);
        if interactions.is_empty() {
            return Ok((0, 0));
        }

        let session_id = interactions[0].session_id.clone();
        let display_id = if config.extraction.prompts.hash_session_ids {
            format!("session:{:016x}", hash_str(&session_id))
        } else {
            session_id.clone()
        };

        // Check if session already processed (by content hash).
        let content_hash = hash_str(&content);
        if let Ok(Some(existing)) = store
            .get_node_by_name(NodeKind::AgentSession, &display_id)
            .await
        {
            if existing.content_hash == Some(content_hash) {
                return Ok((0, 0));
            }
        }

        let metadata = build_session_metadata(&interactions);

        // Create AgentSession node.
        let session_node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::AgentSession,
                name: display_id,
                content_hash: Some(content_hash),
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;

        let edges = self
            .create_session_edges(store, config, &interactions, &session_id, session_node_id)
            .await?;

        Ok(edges)
    }

    async fn create_session_edges(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        interactions: &[AgentInteraction],
        session_id: &str,
        session_node_id: NodeId,
    ) -> crate::error::Result<(u64, u64)> {
        let all_referenced: HashSet<String> = interactions
            .iter()
            .flat_map(|i| i.referenced_files.iter().cloned())
            .collect();
        let all_modified: HashSet<String> = interactions
            .iter()
            .flat_map(|i| i.modified_files.iter().cloned())
            .collect();

        let mut nodes = 1u64;
        let mut edges = 0u64;

        // PromptReferences edges: session → referenced files.
        for file_path in &all_referenced {
            if let Ok(Some(file_node)) = store.get_node_by_name(NodeKind::File, file_path).await {
                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::PromptReferences,
                        members: vec![
                            HyperedgeMember {
                                node_id: session_node_id,
                                role: "session".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: file_node.id,
                                role: "file".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 0.9,
                        last_updated: Utc::now(),
                        metadata: HashMap::new(),
                    })
                    .await?;
                edges += 1;
            }
        }

        // PromptModifiedFiles edges: session → modified files.
        for file_path in &all_modified {
            if let Ok(Some(file_node)) = store.get_node_by_name(NodeKind::File, file_path).await {
                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::PromptModifiedFiles,
                        members: vec![
                            HyperedgeMember {
                                node_id: session_node_id,
                                role: "session".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: file_node.id,
                                role: "file".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: 1.0,
                        last_updated: Utc::now(),
                        metadata: HashMap::new(),
                    })
                    .await?;
                edges += 1;
            }
        }

        // Optionally create individual Prompt nodes per interaction.
        if config.extraction.prompts.store_full_text {
            for interaction in interactions {
                let prompt_name = format!("{session_id}:{}", interaction.timestamp.timestamp());
                let mut prompt_meta = HashMap::new();
                prompt_meta.insert(
                    "referenced_files".to_string(),
                    serde_json::json!(interaction.referenced_files),
                );
                prompt_meta.insert(
                    "modified_files".to_string(),
                    serde_json::json!(interaction.modified_files),
                );
                prompt_meta.insert(
                    "had_correction".to_string(),
                    serde_json::json!(interaction.had_correction),
                );

                store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::Prompt,
                        name: prompt_name,
                        content_hash: None,
                        last_extracted: Utc::now(),
                        metadata: prompt_meta,
                    })
                    .await?;
                nodes += 1;
            }
        }

        Ok((nodes, edges))
    }

    /// Correlate agent sessions with git commits by shared modified files.
    async fn correlate_sessions_with_commits(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
    ) {
        let filter = crate::types::NodeFilter {
            kind: Some(NodeKind::AgentSession),
            ..Default::default()
        };
        let Ok(sessions) = store.find_nodes(&filter).await else {
            return;
        };

        let commit_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Commit),
            ..Default::default()
        };
        let Ok(commits) = store.find_nodes(&commit_filter).await else {
            return;
        };

        if sessions.is_empty() || commits.is_empty() {
            return;
        }

        for session in &sessions {
            correlate_one_session(store, stats, session, &commits).await;
        }
    }
}

async fn correlate_one_session(
    store: &dyn HomerStore,
    stats: &mut ExtractStats,
    session: &Node,
    commits: &[Node],
) {
    let session_files = extract_string_set(&session.metadata, "modified_files");
    if session_files.is_empty() {
        return;
    }

    let session_ts = session
        .metadata
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    for commit in commits {
        let commit_files = extract_string_set(&commit.metadata, "files_changed");
        let shared: Vec<&String> = session_files.intersection(&commit_files).collect();
        if shared.is_empty() {
            continue;
        }

        // Time proximity: session within 24h before commit
        if let Some(s_ts) = session_ts {
            let diff = commit.last_extracted - s_ts;
            if diff.num_hours() < 0 || diff.num_hours() > 24 {
                continue;
            }
        }

        let confidence = shared.len() as f64 / session_files.len().max(1) as f64;
        let mut meta = HashMap::new();
        meta.insert(
            "shared_files".to_string(),
            serde_json::json!(shared.into_iter().collect::<Vec<_>>()),
        );

        if let Err(e) = store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::RelatedPrompts,
                members: vec![
                    HyperedgeMember {
                        node_id: session.id,
                        role: "session".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: commit.id,
                        role: "commit".to_string(),
                        position: 1,
                    },
                ],
                confidence,
                last_updated: Utc::now(),
                metadata: meta,
            })
            .await
        {
            warn!(error = %e, "Failed to create session-commit correlation");
        } else {
            stats.edges_created += 1;
        }
    }
}

fn extract_string_set(metadata: &HashMap<String, serde_json::Value>, key: &str) -> HashSet<String> {
    metadata
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ── JSONL parsing ────────────────────────────────────────────────

fn build_session_metadata(interactions: &[AgentInteraction]) -> HashMap<String, serde_json::Value> {
    let total_interactions = interactions.len();
    let corrections: usize = interactions.iter().filter(|i| i.had_correction).count();
    let all_referenced: HashSet<&str> = interactions
        .iter()
        .flat_map(|i| i.referenced_files.iter().map(String::as_str))
        .collect();
    let all_modified: HashSet<&str> = interactions
        .iter()
        .flat_map(|i| i.modified_files.iter().map(String::as_str))
        .collect();
    let total_tool_uses: u32 = interactions.iter().map(|i| i.tool_uses).sum();
    let earliest = interactions
        .iter()
        .map(|i| i.timestamp)
        .min()
        .unwrap_or_else(Utc::now);

    let mut metadata = HashMap::new();
    metadata.insert("source".to_string(), serde_json::json!("claude-code"));
    metadata.insert(
        "interaction_count".to_string(),
        serde_json::json!(total_interactions),
    );
    metadata.insert(
        "correction_count".to_string(),
        serde_json::json!(corrections),
    );
    metadata.insert(
        "correction_rate".to_string(),
        serde_json::json!(if total_interactions > 0 {
            corrections as f64 / total_interactions as f64
        } else {
            0.0
        }),
    );
    metadata.insert("tool_uses".to_string(), serde_json::json!(total_tool_uses));
    metadata.insert(
        "files_referenced".to_string(),
        serde_json::json!(all_referenced.len()),
    );
    metadata.insert(
        "files_modified".to_string(),
        serde_json::json!(all_modified.len()),
    );
    metadata.insert(
        "timestamp".to_string(),
        serde_json::json!(earliest.to_rfc3339()),
    );
    metadata
}

fn find_session_files(claude_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // Claude Code stores sessions as JSONL files in .claude/projects/*/
    let pattern = claude_dir.join("projects/**/*.jsonl");
    if let Ok(paths) = glob::glob(&pattern.to_string_lossy()) {
        for entry in paths.flatten() {
            if entry.is_file() {
                files.push(entry);
            }
        }
    }

    // Also check direct session files in .claude/
    let direct = claude_dir.join("*.jsonl");
    if let Ok(paths) = glob::glob(&direct.to_string_lossy()) {
        for entry in paths.flatten() {
            if entry.is_file() {
                files.push(entry);
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

fn parse_claude_code_jsonl(content: &str) -> Vec<AgentInteraction> {
    let mut interactions = Vec::new();
    let session_id = format!("{:016x}", hash_str(content));

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
            continue;
        }

        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            i += 1;
            continue;
        };

        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");

        // We primarily care about assistant messages with tool_use content.
        if role == "assistant" {
            let mut referenced = Vec::new();
            let mut modified = Vec::new();
            let mut tool_count = 0u32;

            // Extract tool_use blocks from content array.
            if let Some(content_arr) = msg.get("content").and_then(|v| v.as_array()) {
                for block in content_arr {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if block_type == "tool_use" {
                        tool_count += 1;
                        let tool_name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        if let Some(input) = block.get("input") {
                            extract_tool_file_refs(
                                tool_name,
                                input,
                                &mut referenced,
                                &mut modified,
                            );
                        }
                    }
                }
            }

            let timestamp = msg
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));

            // Correction detection: did the user follow up correcting this action?
            let current_modified: HashSet<String> = modified.iter().cloned().collect();
            let had_correction = detect_correction(&lines, i, &current_modified);

            if !referenced.is_empty() || !modified.is_empty() || tool_count > 0 {
                interactions.push(AgentInteraction {
                    session_id: session_id.clone(),
                    referenced_files: referenced,
                    modified_files: modified,
                    timestamp,
                    had_correction,
                    tool_uses: tool_count,
                });
            }
        }

        i += 1;
    }

    interactions
}

/// Extract file references from a `tool_use` input block.
fn extract_tool_file_refs(
    tool_name: &str,
    input: &serde_json::Value,
    referenced: &mut Vec<String>,
    modified: &mut Vec<String>,
) {
    match tool_name {
        "Read" | "read_file" => {
            if let Some(path) = input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
            {
                referenced.push(normalize_file_path(path));
            }
        }
        "Edit" | "edit_file" | "Write" | "write_file" => {
            if let Some(path) = input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
            {
                let normalized = normalize_file_path(path);
                modified.push(normalized.clone());
                referenced.push(normalized);
            }
        }
        "Grep" | "Glob" | "search" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                referenced.push(normalize_file_path(path));
            }
        }
        _ => {}
    }
}

/// Heuristic correction detection: look ahead for user messages that
/// contain correction markers or reference the same files just modified.
fn detect_correction(
    lines: &[&str],
    assistant_idx: usize,
    current_modified: &HashSet<String>,
) -> bool {
    // Look at the next few lines for a user message.
    for line in lines.iter().skip(assistant_idx + 1).take(3) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" {
            break;
        }

        let text = msg
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| {
                msg.get("content")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|b| b.get("text"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("");

        // Check for explicit correction indicators.
        let text_lower = text.to_lowercase();
        let has_correction_marker = text_lower.contains("no,")
            || text_lower.contains("wrong")
            || text_lower.contains("that's not")
            || text_lower.contains("revert")
            || text_lower.contains("undo")
            || text_lower.contains("instead")
            || text_lower.contains("actually");

        if has_correction_marker {
            return true;
        }

        // User references the same files that were just modified.
        if !current_modified.is_empty() && current_modified.iter().any(|f| text.contains(f)) {
            return true;
        }

        break; // Only check the immediate next user message.
    }
    false
}

// ── Helpers ──────────────────────────────────────────────────────

fn classify_rule_source(path: &Path) -> &'static str {
    let s = path.to_string_lossy();
    if s.contains("CLAUDE.md") || s.contains(".claude/") {
        "claude-code"
    } else if s.contains(".cursor/") {
        "cursor"
    } else if s.contains(".windsurf/") {
        "windsurf"
    } else if s.contains(".clinerules/") {
        "cline"
    } else if s.contains("AGENTS.md") {
        "agents-md"
    } else {
        "unknown"
    }
}

/// Extract file path references from rule/doc content.
fn extract_file_references(content: &str) -> Vec<String> {
    let mut refs = HashSet::new();

    for line in content.lines() {
        // Backtick paths: `src/foo.rs`
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            if let Some(end) = after.find('`') {
                let inside = &after[..end];
                if looks_like_source_path(inside) {
                    refs.insert(normalize_file_path(inside));
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }

    let mut sorted: Vec<String> = refs.into_iter().collect();
    sorted.sort();
    sorted
}

fn looks_like_source_path(s: &str) -> bool {
    if s.len() < 3 || s.len() > 200 {
        return false;
    }
    let has_slash = s.contains('/');
    let has_ext = s.contains('.');
    let no_spaces = !s.contains(' ');
    let no_url = !s.starts_with("http") && !s.starts_with("mailto:");

    (has_slash || has_ext) && no_spaces && no_url
}

fn normalize_file_path(path: &str) -> String {
    let cleaned = path.strip_prefix("./").unwrap_or(path);
    // Strip absolute path prefix if it looks like a repo path.
    // Keep relative paths only.
    if cleaned.starts_with('/') {
        // Try to find a common src/ or similar anchor.
        if let Some(idx) = cleaned.find("/src/") {
            return cleaned[idx + 1..].to_string();
        }
        // Fallback: take the last few components.
        let parts: Vec<&str> = cleaned.split('/').collect();
        if parts.len() > 3 {
            return parts[parts.len() - 3..].join("/");
        }
    }
    cleaned.replace('\\', "/")
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    #[test]
    fn classify_rule_sources() {
        assert_eq!(classify_rule_source(Path::new("CLAUDE.md")), "claude-code");
        assert_eq!(
            classify_rule_source(Path::new(".cursor/rules/my.mdc")),
            "cursor"
        );
        assert_eq!(
            classify_rule_source(Path::new(".windsurf/rules/r.md")),
            "windsurf"
        );
        assert_eq!(classify_rule_source(Path::new(".clinerules/r.md")), "cline");
        assert_eq!(classify_rule_source(Path::new("AGENTS.md")), "agents-md");
        assert_eq!(classify_rule_source(Path::new("other.txt")), "unknown");
    }

    #[test]
    fn extract_file_refs_from_content() {
        let content = "Use `src/main.rs` for the entry point.\nSee `src/lib.rs` for the API.\n";
        let refs = extract_file_references(content);
        assert_eq!(refs, vec!["src/lib.rs", "src/main.rs"]);
    }

    #[test]
    fn extract_file_refs_ignores_non_paths() {
        let content = "Use `cargo build` to compile.\nTry `--verbose` flag.\n";
        let refs = extract_file_references(content);
        assert!(refs.is_empty());
    }

    #[test]
    fn parse_session_jsonl() {
        let jsonl = r#"{"role":"user","content":"read src/main.rs"}
{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}
{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}]}
{"role":"user","content":"actually, revert that"}
"#;

        let interactions = parse_claude_code_jsonl(jsonl);
        assert_eq!(interactions.len(), 2);

        // First interaction reads src/main.rs.
        assert!(
            interactions[0]
                .referenced_files
                .contains(&"src/main.rs".to_string())
        );

        // Second interaction modifies src/lib.rs, and the follow-up has "revert".
        assert!(
            interactions[1]
                .modified_files
                .contains(&"src/lib.rs".to_string())
        );
        assert!(interactions[1].had_correction);
    }

    #[test]
    fn correction_detection_markers() {
        let lines = [
            r#"{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/main.rs","old_string":"a","new_string":"b"}}]}"#,
            r#"{"role":"user","content":"no, that's wrong"}"#,
        ];
        let prev: HashSet<String> = HashSet::new();
        assert!(detect_correction(&lines, 0, &prev));
    }

    #[test]
    fn correction_detection_no_markers() {
        let lines = [
            r#"{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}"#,
            r#"{"role":"user","content":"now add a test for it"}"#,
        ];
        let prev: HashSet<String> = HashSet::new();
        assert!(!detect_correction(&lines, 0, &prev));
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_file_path("./src/main.rs"), "src/main.rs");
        assert_eq!(normalize_file_path("src\\lib.rs"), "src/lib.rs");
        assert_eq!(
            normalize_file_path("/home/user/project/src/main.rs"),
            "src/main.rs"
        );
    }

    #[tokio::test]
    async fn extract_agent_rules_from_fixture() {
        let tmp = tempfile::tempdir().unwrap();

        // Create agent rule files.
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            "# Claude Rules\n\nUse `src/main.rs` as entry point.\nPrefer snake_case.\n",
        )
        .unwrap();

        // Create a source file for cross-reference.
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let store = SqliteStore::in_memory().unwrap();

        // First insert the File node so cross-reference works.
        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/main.rs".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let extractor = PromptExtractor::new(tmp.path());
        let config = HomerConfig::default();
        let stats = extractor.extract(&store, &config).await.unwrap();

        // Should find CLAUDE.md as an agent rule.
        assert!(stats.nodes_created >= 1, "Should create AgentRule node");

        // Verify the AgentRule node.
        let filter = crate::types::NodeFilter {
            kind: Some(NodeKind::AgentRule),
            ..Default::default()
        };
        let rules = store.find_nodes(&filter).await.unwrap();
        assert!(!rules.is_empty(), "Should have AgentRule nodes");
        assert!(
            rules.iter().any(|r| r.name.contains("CLAUDE.md")),
            "Should find CLAUDE.md rule"
        );

        // Should create a PromptReferences edge to src/main.rs.
        assert!(stats.edges_created >= 1, "Should create reference edges");
    }

    #[tokio::test]
    async fn extract_skips_unchanged_rules() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "# Rules\n").unwrap();

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = PromptExtractor::new(tmp.path());

        // First extraction.
        let stats1 = extractor.extract(&store, &config).await.unwrap();
        assert_eq!(stats1.nodes_created, 1);

        // Second extraction — same content, should skip.
        let stats2 = extractor.extract(&store, &config).await.unwrap();
        assert_eq!(stats2.nodes_created, 0, "Should skip unchanged rule");
    }

    #[tokio::test]
    async fn prompt_extraction_disabled_by_default() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a .claude/ dir with a session file.
        std::fs::create_dir_all(tmp.path().join(".claude/projects/test")).unwrap();
        std::fs::write(
            tmp.path().join(".claude/projects/test/session.jsonl"),
            r#"{"role":"user","content":"hello"}"#,
        )
        .unwrap();

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default(); // enabled = false
        let extractor = PromptExtractor::new(tmp.path());

        let _stats = extractor.extract(&store, &config).await.unwrap();

        // Should not create any session nodes (prompts disabled).
        let filter = crate::types::NodeFilter {
            kind: Some(NodeKind::AgentSession),
            ..Default::default()
        };
        let sessions = store.find_nodes(&filter).await.unwrap();
        assert!(
            sessions.is_empty(),
            "Should not extract sessions when disabled"
        );
    }
}
