// Topos spec renderer — produces `.tps` specification files from Homer analysis.
//
// Maps Homer data to Topos constructs:
// - Community clusters  → `# Design` sections
// - High-salience types → `Concept` blocks
// - ADR documents       → `# Principles`
// - Issues/PRs          → `# Requirements`
// - Semantic summaries  → descriptions and invariants

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::fmt::Write as _;

use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::contracts;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, NodeFilter, NodeKind};

use super::traits::Renderer;

#[derive(Debug)]
pub struct ToposSpecRenderer;

#[async_trait::async_trait]
impl Renderer for ToposSpecRenderer {
    fn name(&self) -> &'static str {
        "topos_spec"
    }

    fn output_path(&self) -> &'static str {
        "spec/homer-spec.tps"
    }

    #[instrument(skip_all, name = "topos_spec_render")]
    async fn render(
        &self,
        db: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<String> {
        let mut out = String::new();
        render_spec(&mut out, db, config).await?;
        Ok(out)
    }
}

async fn render_spec(
    out: &mut String,
    db: &dyn HomerStore,
    _config: &HomerConfig,
) -> crate::error::Result<()> {
    // Derive spec name from root module
    let root_name = if let Some(root_id) = contracts::find_root_module_id(db).await? {
        db.get_node(root_id)
            .await?
            .map_or_else(|| "project".to_string(), |m| m.name)
    } else {
        "project".to_string()
    };

    let spec_id = sanitize_identifier(&root_name);
    let _ = writeln!(out, "spec {spec_id}");

    render_principles(out, db).await?;
    render_design(out, db).await?;
    render_concepts(out, db).await?;
    render_requirements(out, db).await?;

    let type_count = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Type),
            ..Default::default()
        })
        .await?
        .len();
    let issue_count = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Issue),
            ..Default::default()
        })
        .await?
        .len();

    info!(
        types = type_count,
        issues = issue_count,
        "Topos spec rendered"
    );

    Ok(())
}

// ── Principles ──────────────────────────────────────────────────────────

async fn render_principles(out: &mut String, db: &dyn HomerStore) -> crate::error::Result<()> {
    let docs = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Document),
            ..Default::default()
        })
        .await?;

    let adrs: Vec<_> = docs
        .iter()
        .filter(|d| {
            d.metadata
                .get("doc_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|t| t == "Adr")
        })
        .collect();

    if adrs.is_empty() {
        return Ok(());
    }

    out.push_str("\n# Principles\n");

    for adr in &adrs {
        let title = adr
            .metadata
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&adr.name);

        let status = adr
            .metadata
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("accepted");

        let _ = write!(out, "\n- {title}");
        if status != "accepted" {
            let _ = write!(out, " [{status}]");
        }
        out.push('\n');

        let summaries = db
            .get_analysis(adr.id, AnalysisKind::SemanticSummary)
            .await?;
        if let Some(summary) = summaries {
            if let Some(text) = summary
                .data
                .get("summary")
                .and_then(serde_json::Value::as_str)
            {
                let _ = writeln!(out, "  context: {text}");
            }
        }
    }

    Ok(())
}

// ── Design ──────────────────────────────────────────────────────────────

async fn render_design(out: &mut String, db: &dyn HomerStore) -> crate::error::Result<()> {
    let community_results = db
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await?;

    if community_results.is_empty() {
        return Ok(());
    }

    // Group files by community
    let mut communities: HashMap<u64, Vec<(String, f64)>> = HashMap::new();
    let files = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::File),
            ..Default::default()
        })
        .await?;

    for result in &community_results {
        let community_id = result
            .data
            .get("community_id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if let Some(node) = files.iter().find(|n| n.id == result.node_id) {
            let sal = result
                .data
                .get("salience")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            communities
                .entry(community_id)
                .or_default()
                .push((node.name.clone(), sal));
        }
    }

    if communities.is_empty() {
        return Ok(());
    }

    out.push_str("\n# Design\n");

    let mut sorted: Vec<_> = communities.into_iter().collect();
    sorted.sort_by_key(|(id, _)| *id);

    for (community_id, mut members) in sorted {
        members.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let area_name = infer_area_name(&members);
        let _ = writeln!(out, "\n## Area {community_id}: {area_name}");

        out.push_str("files:\n");
        for (path, _) in members.iter().take(10) {
            let _ = writeln!(out, "  - {path}");
        }
        if members.len() > 10 {
            let _ = writeln!(out, "  # ... and {} more", members.len() - 10);
        }
    }

    Ok(())
}

/// Infer an area name from the common path prefix of member files.
fn infer_area_name(members: &[(String, f64)]) -> String {
    if members.is_empty() {
        return "Unknown".to_string();
    }
    if members.len() == 1 {
        return leaf_name(&members[0].0);
    }

    let paths: Vec<&str> = members.iter().map(|(p, _)| p.as_str()).collect();
    let prefix = common_path_prefix(&paths);

    if prefix.is_empty() {
        leaf_name(&members[0].0)
    } else {
        let trimmed = prefix.trim_end_matches('/');
        trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
    }
}

fn leaf_name(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn common_path_prefix(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let first = paths[0];
    let mut prefix_len = first.len();

    for path in &paths[1..] {
        prefix_len = first
            .chars()
            .zip(path.chars())
            .take(prefix_len)
            .take_while(|(a, b)| a == b)
            .count();
    }

    let prefix = &first[..prefix_len];
    if let Some(pos) = prefix.rfind('/') {
        first[..=pos].to_string()
    } else {
        String::new()
    }
}

// ── Concepts ────────────────────────────────────────────────────────────

async fn render_concepts(out: &mut String, db: &dyn HomerStore) -> crate::error::Result<()> {
    let type_nodes = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Type),
            ..Default::default()
        })
        .await?;

    if type_nodes.is_empty() {
        return Ok(());
    }

    let salience_results = db
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;
    let salience_map: HashMap<_, _> = salience_results
        .iter()
        .filter_map(|r| {
            let val = r.data.get("score").and_then(serde_json::Value::as_f64)?;
            Some((r.node_id, val))
        })
        .collect();

    let mut entries: Vec<_> = type_nodes
        .iter()
        .map(|t| {
            let sal = salience_map.get(&t.id).copied().unwrap_or(0.0);
            (t, sal)
        })
        .collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top = entries.into_iter().take(20).collect::<Vec<_>>();
    if top.is_empty() {
        return Ok(());
    }

    out.push_str("\n# Concepts\n");

    for (tnode, sal) in &top {
        let name = &tnode.name;
        let short = name.rsplit("::").next().unwrap_or(name);

        let _ = writeln!(out, "\nConcept {short}:");

        if let Some(file) = tnode
            .metadata
            .get("file")
            .and_then(serde_json::Value::as_str)
        {
            let _ = writeln!(out, "  file: {file}");
        }

        if *sal > 0.0 {
            let _ = writeln!(out, "  salience: {sal:.2}");
        }

        if let Some(doc) = tnode
            .metadata
            .get("doc_comment")
            .and_then(serde_json::Value::as_str)
        {
            let first_line = doc.lines().next().unwrap_or(doc);
            let _ = writeln!(out, "  description: {first_line}");
        }

        let summary = db
            .get_analysis(tnode.id, AnalysisKind::SemanticSummary)
            .await?;
        if let Some(s) = summary {
            if let Some(text) = s.data.get("summary").and_then(serde_json::Value::as_str) {
                let _ = writeln!(out, "  summary: {text}");
            }
        }

        let invariant = db
            .get_analysis(tnode.id, AnalysisKind::InvariantDescription)
            .await?;
        if let Some(inv) = invariant {
            if let Some(text) = inv
                .data
                .get("invariant")
                .and_then(serde_json::Value::as_str)
            {
                let _ = writeln!(out, "  invariant: {text}");
            }
        }
    }

    Ok(())
}

// ── Requirements ────────────────────────────────────────────────────────

async fn render_requirements(out: &mut String, db: &dyn HomerStore) -> crate::error::Result<()> {
    let issues = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Issue),
            ..Default::default()
        })
        .await?;

    let prs = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::PullRequest),
            ..Default::default()
        })
        .await?;

    if issues.is_empty() && prs.is_empty() {
        return Ok(());
    }

    out.push_str("\n# Requirements\n");

    for issue in issues.iter().take(50) {
        let title = issue
            .metadata
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&issue.name);
        let state = issue
            .metadata
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("open");

        let _ = writeln!(out, "\n## {}: {title}", issue.name);
        let _ = writeln!(out, "status: {state}");

        let resolving: Vec<_> = prs
            .iter()
            .filter(|pr| {
                pr.metadata
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|body| {
                        let lower = body.to_lowercase();
                        issue
                            .metadata
                            .get("number")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|num| lower.contains(&format!("#{num}")))
                    })
            })
            .collect();

        if !resolving.is_empty() {
            out.push_str("evidence:\n");
            for pr in &resolving {
                let pr_title = pr
                    .metadata
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&pr.name);
                let _ = writeln!(out, "  - {}: {pr_title}", pr.name);
            }
        }
    }

    // Merged PRs as tasks
    let merged: Vec<_> = prs
        .iter()
        .filter(|pr| {
            pr.metadata
                .get("merged_at")
                .and_then(serde_json::Value::as_str)
                .is_some()
        })
        .take(20)
        .collect();

    if !merged.is_empty() {
        out.push_str("\n# Tasks\n");
        for pr in &merged {
            let title = pr
                .metadata
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&pr.name);
            let _ = writeln!(out, "\n## {}: {title}", pr.name);
            out.push_str("status: done\n");
            if let Some(at) = pr
                .metadata
                .get("merged_at")
                .and_then(serde_json::Value::as_str)
            {
                let _ = writeln!(out, "merged: {at}");
            }
        }
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn sanitize_identifier(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "project".to_string()
    } else if cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{AnalysisResult, AnalysisResultId, Node, NodeId};
    use chrono::Utc;

    #[test]
    fn sanitize_identifier_basic() {
        assert_eq!(sanitize_identifier("my-project"), "my_project");
        assert_eq!(sanitize_identifier("."), "_");
        assert_eq!(sanitize_identifier("123abc"), "_123abc");
        assert_eq!(sanitize_identifier("foo_bar"), "foo_bar");
    }

    #[test]
    fn common_prefix_paths() {
        let paths = vec![
            "src/auth/login.rs",
            "src/auth/logout.rs",
            "src/auth/token.rs",
        ];
        assert_eq!(common_path_prefix(&paths), "src/auth/");
    }

    #[test]
    fn common_prefix_no_common() {
        let paths = vec!["src/foo.rs", "lib/bar.rs"];
        assert_eq!(common_path_prefix(&paths), "");
    }

    #[test]
    fn infer_area_from_members() {
        let members = vec![
            ("src/auth/login.rs".to_string(), 0.9),
            ("src/auth/token.rs".to_string(), 0.5),
        ];
        assert_eq!(infer_area_name(&members), "auth");
    }

    #[tokio::test]
    async fn renders_spec_header() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: ".".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let renderer = ToposSpecRenderer;
        let config = HomerConfig::default();
        let content = renderer.render(&store, &config).await.unwrap();
        assert!(
            content.starts_with("spec _"),
            "Should start with spec identifier: {content}"
        );
    }

    #[tokio::test]
    async fn renders_concepts_from_types() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: "myproject".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let type_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Type,
                name: "User".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("file".to_string(), serde_json::json!("src/models/user.rs"));
                    m.insert(
                        "doc_comment".to_string(),
                        serde_json::json!("A user account in the system."),
                    );
                    m
                },
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: type_id,
                kind: AnalysisKind::CompositeSalience,
                data: serde_json::json!({ "score": 0.85 }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        let renderer = ToposSpecRenderer;
        let config = HomerConfig::default();
        let content = renderer.render(&store, &config).await.unwrap();

        assert!(content.contains("spec myproject"), "content: {content}");
        assert!(content.contains("# Concepts"), "content: {content}");
        assert!(content.contains("Concept User:"), "content: {content}");
        assert!(
            content.contains("file: src/models/user.rs"),
            "content: {content}"
        );
        assert!(
            content.contains("description: A user account"),
            "content: {content}"
        );
    }

    #[tokio::test]
    async fn renders_requirements_from_issues() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: ".".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Issue,
                name: "Issue#42".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("title".to_string(), serde_json::json!("Add login page"));
                    m.insert("state".to_string(), serde_json::json!("closed"));
                    m.insert("number".to_string(), serde_json::json!(42));
                    m
                },
            })
            .await
            .unwrap();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::PullRequest,
                name: "PR#50".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert(
                        "title".to_string(),
                        serde_json::json!("Implement login page"),
                    );
                    m.insert("body".to_string(), serde_json::json!("Fixes #42"));
                    m.insert(
                        "merged_at".to_string(),
                        serde_json::json!("2025-01-15T10:00:00Z"),
                    );
                    m
                },
            })
            .await
            .unwrap();

        let renderer = ToposSpecRenderer;
        let config = HomerConfig::default();
        let content = renderer.render(&store, &config).await.unwrap();

        assert!(content.contains("# Requirements"), "content: {content}");
        assert!(
            content.contains("Issue#42: Add login page"),
            "content: {content}"
        );
        assert!(content.contains("status: closed"), "content: {content}");
        assert!(content.contains("evidence:"), "content: {content}");
        assert!(
            content.contains("PR#50: Implement login page"),
            "content: {content}"
        );
    }

    #[tokio::test]
    async fn renders_principles_from_adrs() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: ".".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Document,
                name: "docs/adr/001-use-sqlite.md".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("doc_type".to_string(), serde_json::json!("Adr"));
                    m.insert(
                        "title".to_string(),
                        serde_json::json!("Use SQLite for local storage"),
                    );
                    m.insert("status".to_string(), serde_json::json!("accepted"));
                    m
                },
            })
            .await
            .unwrap();

        let renderer = ToposSpecRenderer;
        let config = HomerConfig::default();
        let content = renderer.render(&store, &config).await.unwrap();

        assert!(content.contains("# Principles"), "content: {content}");
        assert!(
            content.contains("Use SQLite for local storage"),
            "content: {content}"
        );
    }
}
