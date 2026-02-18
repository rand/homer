#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{
    AnalysisKind, AnalysisResult, AnalysisResultId, HyperedgeKind, NodeId, NodeKind,
};

use super::AnalyzeStats;
use super::traits::Analyzer;

#[derive(Debug)]
pub struct TaskPatternAnalyzer;

#[async_trait::async_trait]
impl Analyzer for TaskPatternAnalyzer {
    fn name(&self) -> &'static str {
        "task_pattern"
    }

    #[instrument(skip_all, name = "task_pattern_analyze")]
    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Gather prompt data from the store.
        let prompt_data = collect_prompt_data(store).await?;

        if prompt_data.session_refs.is_empty() {
            info!("No prompt data found, skipping task pattern analysis");
            stats.duration = start.elapsed();
            return Ok(stats);
        }

        info!(
            sessions = prompt_data.session_refs.len(),
            files_referenced = prompt_data.file_ref_counts.len(),
            files_modified = prompt_data.file_mod_counts.len(),
            "Collected prompt data for task pattern analysis"
        );

        // 1. Compute PromptHotspot per file.
        let hotspot_count = compute_prompt_hotspots(store, &prompt_data).await?;
        stats.results_stored += hotspot_count;

        // 2. Compute CorrectionHotspot per file.
        let correction_count = compute_correction_hotspots(store, &prompt_data).await?;
        stats.results_stored += correction_count;

        // 3. Compute TaskPattern on root module.
        let task_count = compute_task_patterns(store, &prompt_data).await?;
        stats.results_stored += task_count;

        // 4. Compute DomainVocabulary on root module.
        let vocab_count = compute_domain_vocabulary(store, &prompt_data).await?;
        stats.results_stored += vocab_count;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Task pattern analysis complete"
        );
        Ok(stats)
    }
}

// ── Data collection ──────────────────────────────────────────────

/// Aggregated prompt data from the store.
#[derive(Debug, Default)]
struct PromptData {
    /// Per-session: list of referenced file `NodeId`s.
    session_refs: Vec<SessionInfo>,
    /// Per-file: how many sessions referenced it.
    file_ref_counts: HashMap<NodeId, u32>,
    /// Per-file: how many sessions modified it.
    file_mod_counts: HashMap<NodeId, u32>,
    /// Per-file: correction count (sessions that had corrections and modified this file).
    file_correction_counts: HashMap<NodeId, u32>,
    /// Per-file: total interaction count involving this file.
    file_interaction_counts: HashMap<NodeId, u32>,
}

#[derive(Debug)]
struct SessionInfo {
    modified_files: Vec<NodeId>,
}

async fn collect_prompt_data(store: &dyn HomerStore) -> crate::error::Result<PromptData> {
    let mut data = PromptData::default();

    // Get all AgentSession nodes.
    let session_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::AgentSession),
        ..Default::default()
    };
    let sessions = store.find_nodes(&session_filter).await?;

    // Get PromptReferences and PromptModifiedFiles edges.
    let ref_edges = store
        .get_edges_by_kind(HyperedgeKind::PromptReferences)
        .await?;
    let mod_edges = store
        .get_edges_by_kind(HyperedgeKind::PromptModifiedFiles)
        .await?;

    // Build session → files maps.
    let mut session_ref_map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for edge in &ref_edges {
        let session = edge.members.iter().find(|m| m.role == "session" || m.role == "rule");
        let file = edge.members.iter().find(|m| m.role == "file");
        if let (Some(s), Some(f)) = (session, file) {
            session_ref_map
                .entry(s.node_id)
                .or_default()
                .push(f.node_id);
        }
    }

    let mut session_mod_map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for edge in &mod_edges {
        let session = edge.members.iter().find(|m| m.role == "session");
        let file = edge.members.iter().find(|m| m.role == "file");
        if let (Some(s), Some(f)) = (session, file) {
            session_mod_map
                .entry(s.node_id)
                .or_default()
                .push(f.node_id);
        }
    }

    for session in &sessions {
        let refs = session_ref_map.get(&session.id).cloned().unwrap_or_default();
        let mods = session_mod_map.get(&session.id).cloned().unwrap_or_default();

        let correction_count = session
            .metadata
            .get("correction_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let interaction_count = session
            .metadata
            .get("interaction_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;

        // Aggregate per-file counts.
        for file_id in &refs {
            *data.file_ref_counts.entry(*file_id).or_default() += 1;
            *data.file_interaction_counts.entry(*file_id).or_default() += interaction_count;
        }
        for file_id in &mods {
            *data.file_mod_counts.entry(*file_id).or_default() += 1;
            if correction_count > 0 {
                *data.file_correction_counts.entry(*file_id).or_default() += correction_count;
            }
        }

        data.session_refs.push(SessionInfo {
            modified_files: mods,
        });
    }

    // Also count references from AgentRule nodes.
    let rule_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::AgentRule),
        ..Default::default()
    };
    let rules = store.find_nodes(&rule_filter).await?;

    for rule in &rules {
        if let Some(refs) = session_ref_map.get(&rule.id) {
            for file_id in refs {
                *data.file_ref_counts.entry(*file_id).or_default() += 1;
            }
        }
    }

    Ok(data)
}

// ── Prompt hotspots ──────────────────────────────────────────────

async fn compute_prompt_hotspots(
    store: &dyn HomerStore,
    data: &PromptData,
) -> crate::error::Result<u64> {
    let mut stored = 0u64;

    for (file_id, &ref_count) in &data.file_ref_counts {
        let mod_count = data.file_mod_counts.get(file_id).copied().unwrap_or(0);

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id: *file_id,
            kind: AnalysisKind::PromptHotspot,
            data: serde_json::json!({
                "reference_count": ref_count,
                "modification_count": mod_count,
                "session_count": ref_count,
            }),
            input_hash: 0,
            computed_at: Utc::now(),
        };

        store.store_analysis(&result).await?;
        stored += 1;
    }

    Ok(stored)
}

// ── Correction hotspots ──────────────────────────────────────────

async fn compute_correction_hotspots(
    store: &dyn HomerStore,
    data: &PromptData,
) -> crate::error::Result<u64> {
    let mut stored = 0u64;

    for (file_id, &correction_count) in &data.file_correction_counts {
        let interaction_count = data
            .file_interaction_counts
            .get(file_id)
            .copied()
            .unwrap_or(1);

        let correction_rate = if interaction_count > 0 {
            f64::from(correction_count) / f64::from(interaction_count)
        } else {
            0.0
        };

        // A file is a "confusion zone" if correction rate exceeds 20%.
        let is_confusion_zone = correction_rate > 0.2 && correction_count >= 2;

        let result = AnalysisResult {
            id: AnalysisResultId(0),
            node_id: *file_id,
            kind: AnalysisKind::CorrectionHotspot,
            data: serde_json::json!({
                "correction_count": correction_count,
                "interaction_count": interaction_count,
                "correction_rate": correction_rate,
                "is_confusion_zone": is_confusion_zone,
            }),
            input_hash: 0,
            computed_at: Utc::now(),
        };

        store.store_analysis(&result).await?;
        stored += 1;
    }

    Ok(stored)
}

// ── Task patterns ────────────────────────────────────────────────

async fn compute_task_patterns(
    store: &dyn HomerStore,
    data: &PromptData,
) -> crate::error::Result<u64> {
    // Group sessions by their modified file sets (fingerprint).
    // Sessions that touch the same set of files likely represent the same "task shape."
    let mut file_set_groups: HashMap<Vec<NodeId>, Vec<usize>> = HashMap::new();

    for (idx, session) in data.session_refs.iter().enumerate() {
        if session.modified_files.is_empty() {
            continue;
        }
        let mut key = session.modified_files.clone();
        key.sort_by_key(|id| id.0);
        key.dedup();
        file_set_groups.entry(key).or_default().push(idx);
    }

    // Filter to groups with 2+ sessions (recurring patterns).
    let mut patterns: Vec<serde_json::Value> = Vec::new();

    for (file_ids, session_indices) in &file_set_groups {
        if session_indices.len() < 2 {
            continue;
        }

        // Resolve file names.
        let mut file_names = Vec::new();
        for fid in file_ids {
            let name = store
                .get_node(*fid)
                .await?
                .map_or_else(|| format!("node:{}", fid.0), |n| n.name);
            file_names.push(name);
        }

        // Infer pattern name from file paths.
        let pattern_name = infer_pattern_name(&file_names);

        patterns.push(serde_json::json!({
            "pattern_name": pattern_name,
            "typical_files": file_names,
            "frequency": session_indices.len(),
        }));
    }

    // Sort by frequency (descending).
    patterns.sort_by(|a, b| {
        let fa = a.get("frequency").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let fb = b.get("frequency").and_then(serde_json::Value::as_u64).unwrap_or(0);
        fb.cmp(&fa)
    });

    // Store on root module node.
    if !patterns.is_empty() {
        if let Some(root_id) = find_root_module(store).await? {
            let result = AnalysisResult {
                id: AnalysisResultId(0),
                node_id: root_id,
                kind: AnalysisKind::TaskPattern,
                data: serde_json::json!({
                    "patterns": patterns,
                    "total_sessions": data.session_refs.len(),
                }),
                input_hash: 0,
                computed_at: Utc::now(),
            };
            store.store_analysis(&result).await?;
            return Ok(1);
        }
    }

    Ok(0)
}

fn infer_pattern_name(file_names: &[String]) -> String {
    if file_names.is_empty() {
        return "unknown task".to_string();
    }

    // Try to identify common directory.
    let dirs: Vec<&str> = file_names
        .iter()
        .filter_map(|f| std::path::Path::new(f).parent())
        .filter_map(|p| p.to_str())
        .collect();

    if !dirs.is_empty() {
        // Find common prefix.
        let first = dirs[0];
        let common = dirs.iter().fold(first, |acc, d| {
            let len = acc
                .chars()
                .zip(d.chars())
                .take_while(|(a, b)| a == b)
                .count();
            &acc[..len]
        });

        let common = common.trim_end_matches('/');
        if !common.is_empty() {
            return format!("modify {common}");
        }
    }

    // Fallback: use the most common file extension.
    let mut ext_counts: HashMap<&str, u32> = HashMap::new();
    for name in file_names {
        if let Some(ext) = std::path::Path::new(name).extension().and_then(|e| e.to_str()) {
            *ext_counts.entry(ext).or_default() += 1;
        }
    }

    if let Some((ext, _)) = ext_counts.iter().max_by_key(|(_, c)| *c) {
        return format!("modify .{ext} files");
    }

    format!("modify {} files", file_names.len())
}

// ── Domain vocabulary ────────────────────────────────────────────

async fn compute_domain_vocabulary(
    store: &dyn HomerStore,
    data: &PromptData,
) -> crate::error::Result<u64> {
    // Build a vocabulary map: file paths → code identifiers (functions/types defined in them).
    // Files frequently referenced in prompts are likely domain-significant.

    let hotspot_files: Vec<NodeId> = data
        .file_ref_counts
        .iter()
        .filter(|(_, count)| **count >= 2)
        .map(|(id, _)| *id)
        .collect();

    if hotspot_files.is_empty() {
        return Ok(0);
    }

    // For each hotspot file, find functions/types defined in it.
    let fn_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let functions = store.find_nodes(&fn_filter).await?;

    let type_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::Type),
        ..Default::default()
    };
    let types = store.find_nodes(&type_filter).await?;

    // Build file → entities map.
    let mut file_entities: HashMap<String, Vec<String>> = HashMap::new();

    for func in &functions {
        if let Some(file) = func.metadata.get("file").and_then(|v| v.as_str()) {
            file_entities
                .entry(file.to_string())
                .or_default()
                .push(func.name.clone());
        }
    }
    for typ in &types {
        if let Some(file) = typ.metadata.get("file").and_then(|v| v.as_str()) {
            file_entities
                .entry(file.to_string())
                .or_default()
                .push(typ.name.clone());
        }
    }

    // Build vocabulary entries: map file names to their domain entities.
    let mut vocabulary: Vec<serde_json::Value> = Vec::new();

    for file_id in &hotspot_files {
        let Some(file_node) = store.get_node(*file_id).await? else {
            continue;
        };

        let ref_count = data.file_ref_counts.get(file_id).copied().unwrap_or(0);
        let entities = file_entities.get(&file_node.name).cloned().unwrap_or_default();

        if entities.is_empty() {
            continue;
        }

        // Extract the "domain term" from the file path (e.g., "auth" from "src/auth.rs").
        let term = extract_domain_term(&file_node.name);

        vocabulary.push(serde_json::json!({
            "term": term,
            "file": file_node.name,
            "entities": entities,
            "reference_count": ref_count,
        }));
    }

    vocabulary.sort_by(|a, b| {
        let ra = a.get("reference_count").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let rb = b.get("reference_count").and_then(serde_json::Value::as_u64).unwrap_or(0);
        rb.cmp(&ra)
    });

    if !vocabulary.is_empty() {
        if let Some(root_id) = find_root_module(store).await? {
            let result = AnalysisResult {
                id: AnalysisResultId(0),
                node_id: root_id,
                kind: AnalysisKind::DomainVocabulary,
                data: serde_json::json!({
                    "vocabulary": vocabulary,
                }),
                input_hash: 0,
                computed_at: Utc::now(),
            };
            store.store_analysis(&result).await?;
            return Ok(1);
        }
    }

    Ok(0)
}

fn extract_domain_term(file_path: &str) -> String {
    let path = std::path::Path::new(file_path);
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_path)
        .to_string()
}

// ── Helpers ──────────────────────────────────────────────────────

async fn find_root_module(store: &dyn HomerStore) -> crate::error::Result<Option<NodeId>> {
    let mod_filter = crate::types::NodeFilter {
        kind: Some(NodeKind::Module),
        ..Default::default()
    };
    let modules = store.find_nodes(&mod_filter).await?;
    Ok(modules.iter().min_by_key(|m| m.name.len()).map(|m| m.id))
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Hyperedge, HyperedgeId, HyperedgeMember, Node};
    use std::collections::HashMap;

    async fn setup_prompt_data(store: &SqliteStore) -> (NodeId, NodeId, NodeId) {
        // Create file nodes.
        let file_a = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/auth.rs".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let file_b = store
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

        // Create module node.
        let root = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: ".".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Create function nodes (for vocabulary).
        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Function,
                name: "src/auth.rs::validate_token".to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("file".to_string(), serde_json::json!("src/auth.rs"));
                    m
                },
            })
            .await
            .unwrap();

        // Create 3 sessions, all referencing auth.rs.
        for i in 0..3 {
            let mut meta = HashMap::new();
            meta.insert("source".to_string(), serde_json::json!("claude-code"));
            meta.insert("interaction_count".to_string(), serde_json::json!(5));
            meta.insert(
                "correction_count".to_string(),
                serde_json::json!(if i == 0 { 2 } else { 0 }),
            );

            let session = store
                .upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::AgentSession,
                    name: format!("session:{i}"),
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata: meta,
                })
                .await
                .unwrap();

            // PromptReferences: session → auth.rs
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::PromptReferences,
                    members: vec![
                        HyperedgeMember {
                            node_id: session,
                            role: "session".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: file_a,
                            role: "file".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 0.9,
                    last_updated: Utc::now(),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();

            // PromptModifiedFiles: session → auth.rs
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::PromptModifiedFiles,
                    members: vec![
                        HyperedgeMember {
                            node_id: session,
                            role: "session".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: file_a,
                            role: "file".to_string(),
                            position: 1,
                        },
                    ],
                    confidence: 1.0,
                    last_updated: Utc::now(),
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();
        }

        (file_a, file_b, root)
    }

    #[tokio::test]
    async fn prompt_hotspot_computation() {
        let store = SqliteStore::in_memory().unwrap();
        let (file_a, _file_b, _root) = setup_prompt_data(&store).await;

        let config = HomerConfig::default();
        let analyzer = TaskPatternAnalyzer;
        let stats = analyzer.analyze(&store, &config).await.unwrap();

        assert!(stats.results_stored > 0, "Should store analysis results");

        // Check PromptHotspot for auth.rs.
        let hotspot = store
            .get_analysis(file_a, AnalysisKind::PromptHotspot)
            .await
            .unwrap();
        assert!(hotspot.is_some(), "Should have PromptHotspot for auth.rs");
        let hs = hotspot.unwrap();
        let ref_count = hs.data.get("reference_count").and_then(|v| v.as_u64()).unwrap_or(0);
        assert_eq!(ref_count, 3, "Should have 3 references from 3 sessions");
    }

    #[tokio::test]
    async fn correction_hotspot_computation() {
        let store = SqliteStore::in_memory().unwrap();
        let (file_a, _file_b, _root) = setup_prompt_data(&store).await;

        let config = HomerConfig::default();
        let analyzer = TaskPatternAnalyzer;
        analyzer.analyze(&store, &config).await.unwrap();

        // Check CorrectionHotspot for auth.rs.
        let correction = store
            .get_analysis(file_a, AnalysisKind::CorrectionHotspot)
            .await
            .unwrap();
        assert!(
            correction.is_some(),
            "Should have CorrectionHotspot for auth.rs"
        );
        let ch = correction.unwrap();
        let count = ch
            .data
            .get("correction_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert!(count > 0, "Should have corrections");
    }

    #[tokio::test]
    async fn task_pattern_grouping() {
        let store = SqliteStore::in_memory().unwrap();
        let (_file_a, _file_b, root) = setup_prompt_data(&store).await;

        let config = HomerConfig::default();
        let analyzer = TaskPatternAnalyzer;
        analyzer.analyze(&store, &config).await.unwrap();

        // Check TaskPattern on root module.
        let patterns = store
            .get_analysis(root, AnalysisKind::TaskPattern)
            .await
            .unwrap();
        assert!(patterns.is_some(), "Should have TaskPattern on root module");
        let tp = patterns.unwrap();
        let pattern_list = tp.data.get("patterns").and_then(|v| v.as_array());
        assert!(
            pattern_list.is_some(),
            "Should have patterns array"
        );
    }

    #[tokio::test]
    async fn domain_vocabulary_extraction() {
        let store = SqliteStore::in_memory().unwrap();
        let (_file_a, _file_b, root) = setup_prompt_data(&store).await;

        let config = HomerConfig::default();
        let analyzer = TaskPatternAnalyzer;
        analyzer.analyze(&store, &config).await.unwrap();

        // Check DomainVocabulary on root module.
        let vocab = store
            .get_analysis(root, AnalysisKind::DomainVocabulary)
            .await
            .unwrap();
        assert!(
            vocab.is_some(),
            "Should have DomainVocabulary on root module"
        );
        let dv = vocab.unwrap();
        let entries = dv.data.get("vocabulary").and_then(|v| v.as_array());
        assert!(entries.is_some(), "Should have vocabulary array");
        let entries = entries.unwrap();
        assert!(
            entries.iter().any(|e| {
                e.get("term")
                    .and_then(|v| v.as_str())
                    .is_some_and(|t| t == "auth")
            }),
            "Should extract 'auth' as a domain term"
        );
    }

    #[tokio::test]
    async fn no_prompt_data_skips_analysis() {
        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let analyzer = TaskPatternAnalyzer;
        let stats = analyzer.analyze(&store, &config).await.unwrap();
        assert_eq!(stats.results_stored, 0, "Should store nothing with no data");
    }

    #[test]
    fn infer_pattern_name_from_files() {
        assert_eq!(
            infer_pattern_name(&["src/auth/login.rs".into(), "src/auth/token.rs".into()]),
            "modify src/auth"
        );
        assert_eq!(
            infer_pattern_name(&["src/a.rs".into(), "tests/b.rs".into()]),
            "modify .rs files"
        );
    }

    #[test]
    fn extract_domain_term_from_path() {
        assert_eq!(extract_domain_term("src/auth.rs"), "auth");
        assert_eq!(extract_domain_term("lib/pipeline.py"), "pipeline");
        assert_eq!(extract_domain_term("index.ts"), "index");
    }
}
