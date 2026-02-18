// Per-directory `.context.md` renderer.
//
// Produces one `.context.md` for each directory that contains source files,
// giving AI agents focused context about that module.

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;

use tracing::info;

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, HyperedgeKind, NodeFilter, NodeId, NodeKind};

use super::traits::Renderer;

#[derive(Debug)]
pub struct ModuleContextRenderer;

#[async_trait::async_trait]
impl Renderer for ModuleContextRenderer {
    fn name(&self) -> &'static str {
        "module_context"
    }

    fn output_path(&self) -> &'static str {
        ".context.md"
    }

    async fn render(
        &self,
        _store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<String> {
        // Multi-file renderer — real work is in write()
        Ok(String::new())
    }

    async fn write(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        repo_root: &Path,
    ) -> crate::error::Result<()> {
        let modules_written = render_all_module_contexts(store, config, repo_root).await?;
        info!(modules = modules_written, "Module context files written");
        Ok(())
    }
}

// ── Precomputed lookup tables ────────────────────────────────────────

/// An entity name with optional salience score and classification.
type EntityEntry = (String, Option<(f64, String)>);

/// Holds all precomputed data for rendering module contexts.
struct ModuleData {
    salience: HashMap<NodeId, (f64, String)>,
    stability: HashMap<NodeId, String>,
    freq: HashMap<NodeId, u64>,
    bus_factor: HashMap<NodeId, u64>,
    module_files: HashMap<String, Vec<String>>,
    dir_entities: HashMap<String, Vec<EntityEntry>>,
    imports_from: HashMap<String, Vec<String>>,
    imported_by: HashMap<String, Vec<String>>,
    ext_deps: HashMap<String, Vec<String>>,
    file_ids: HashMap<String, NodeId>,
    summaries: HashMap<NodeId, String>,
    doc_refs: HashMap<String, Vec<String>>,
    co_changes: HashMap<String, Vec<String>>,
    recent_commits: HashMap<String, Vec<(String, String)>>,
    naming_convention: Option<String>,
}

// ── Data loading ─────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
async fn load_module_data(db: &dyn HomerStore) -> crate::error::Result<ModuleData> {
    let salience = load_salience_map(db).await?;
    let stability =
        load_string_analysis(db, AnalysisKind::StabilityClassification, "classification").await?;
    let freq = load_u64_analysis(db, AnalysisKind::ChangeFrequency, "total").await?;
    let bus_factor =
        load_u64_analysis(db, AnalysisKind::ContributorConcentration, "bus_factor").await?;

    let module_files = load_belongs_to(db).await?;
    let (imports_from, imported_by) = load_import_relationships(db).await?;
    let ext_deps = load_external_deps(db).await?;
    let dir_entities = load_dir_entities(db, &salience).await?;

    let files = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::File),
            ..Default::default()
        })
        .await?;
    let file_ids: HashMap<String, NodeId> = files.iter().map(|f| (f.name.clone(), f.id)).collect();

    // Semantic summaries (from SemanticSummary analysis)
    let summary_results = db
        .get_analyses_by_kind(AnalysisKind::SemanticSummary)
        .await?;
    let summaries: HashMap<_, _> = summary_results
        .iter()
        .filter_map(|r| {
            let s = r.data.get("summary").and_then(serde_json::Value::as_str)?;
            Some((r.node_id, s.to_string()))
        })
        .collect();

    // Document references per directory
    let doc_edges = db.get_edges_by_kind(HyperedgeKind::Documents).await?;
    let mut doc_refs: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &doc_edges {
        let doc_member = edge.members.iter().find(|m| m.role == "document");
        let entity_member = edge.members.iter().find(|m| m.role == "entity");
        if let (Some(d), Some(e)) = (doc_member, entity_member) {
            let doc_name = db.get_node(d.node_id).await?.map(|n| n.name);
            let entity_name = db.get_node(e.node_id).await?.map(|n| n.name);
            if let (Some(dn), Some(en)) = (doc_name, entity_name) {
                let dir = dir_of(&en).to_string();
                let entry = doc_refs.entry(dir).or_default();
                if !entry.contains(&dn) {
                    entry.push(dn);
                }
            }
        }
    }

    // Co-change partners per directory
    let co_change_edges = db.get_edges_by_kind(HyperedgeKind::CoChanges).await?;
    let mut co_changes: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &co_change_edges {
        let dirs: Vec<String> = edge
            .members
            .iter()
            .filter_map(|m| {
                file_ids
                    .iter()
                    .find(|&(_, id)| *id == m.node_id)
                    .map(|(name, _)| dir_of(name).to_string())
            })
            .collect();
        for dir in &dirs {
            for other in &dirs {
                if dir != other {
                    let entry = co_changes.entry(dir.clone()).or_default();
                    if !entry.contains(other) {
                        entry.push(other.clone());
                    }
                }
            }
        }
    }

    // Recent commits per directory
    let modify_edges = db.get_edges_by_kind(HyperedgeKind::Modifies).await?;
    let mut recent_commits: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut sorted_edges: Vec<_> = modify_edges.iter().collect();
    sorted_edges.sort_by(|a, b| b.last_updated.cmp(&a.last_updated));
    for edge in sorted_edges.iter().take(200) {
        let commit_member = edge.members.iter().find(|m| m.role == "commit");
        let file_members: Vec<_> = edge.members.iter().filter(|m| m.role == "file").collect();
        if let Some(cm) = commit_member {
            let commit_name = db
                .get_node(cm.node_id)
                .await?
                .map_or_else(|| "?".to_string(), |n| n.name);
            let date = edge.last_updated.format("%Y-%m-%d").to_string();
            for fm in &file_members {
                if let Some(fname) = file_ids
                    .iter()
                    .find(|&(_, id)| *id == fm.node_id)
                    .map(|(n, _)| n.clone())
                {
                    let dir = dir_of(&fname).to_string();
                    let entry = recent_commits.entry(dir).or_default();
                    if entry.len() < 5 {
                        let label = format!("{date} {commit_name}");
                        if !entry.iter().any(|(l, _)| l == &label) {
                            entry.push((label, fname));
                        }
                    }
                }
            }
        }
    }

    // Global naming convention
    let mod_filter = NodeFilter {
        kind: Some(NodeKind::Module),
        ..Default::default()
    };
    let modules = db.find_nodes(&mod_filter).await?;
    let root_id = modules.iter().min_by_key(|m| m.name.len()).map(|m| m.id);
    let naming_convention = if let Some(rid) = root_id {
        db.get_analysis(rid, AnalysisKind::NamingPattern)
            .await?
            .and_then(|r| {
                r.data
                    .get("dominant")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
            })
    } else {
        None
    };

    Ok(ModuleData {
        salience,
        stability,
        freq,
        bus_factor,
        module_files,
        dir_entities,
        imports_from,
        imported_by,
        ext_deps,
        file_ids,
        summaries,
        doc_refs,
        co_changes,
        recent_commits,
        naming_convention,
    })
}

async fn load_salience_map(
    db: &dyn HomerStore,
) -> crate::error::Result<HashMap<NodeId, (f64, String)>> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;
    Ok(results
        .iter()
        .filter_map(|r| {
            let val = r.data.get("score")?.as_f64()?;
            let cls = r
                .data
                .get("classification")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown");
            Some((r.node_id, (val, cls.to_string())))
        })
        .collect())
}

async fn load_string_analysis(
    db: &dyn HomerStore,
    kind: AnalysisKind,
    field: &str,
) -> crate::error::Result<HashMap<NodeId, String>> {
    let results = db.get_analyses_by_kind(kind).await?;
    Ok(results
        .iter()
        .filter_map(|r| {
            let val = r.data.get(field).and_then(serde_json::Value::as_str)?;
            Some((r.node_id, val.to_string()))
        })
        .collect())
}

async fn load_u64_analysis(
    db: &dyn HomerStore,
    kind: AnalysisKind,
    field: &str,
) -> crate::error::Result<HashMap<NodeId, u64>> {
    let results = db.get_analyses_by_kind(kind).await?;
    Ok(results
        .iter()
        .filter_map(|r| {
            let val = r.data.get(field)?.as_u64()?;
            Some((r.node_id, val))
        })
        .collect())
}

async fn load_belongs_to(
    db: &dyn HomerStore,
) -> crate::error::Result<HashMap<String, Vec<String>>> {
    let edges = db.get_edges_by_kind(HyperedgeKind::BelongsTo).await?;
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &edges {
        let container = edge.members.iter().find(|m| m.role == "container");
        let member = edge.members.iter().find(|m| m.role == "member");
        if let (Some(c), Some(m)) = (container, member) {
            let cn = db.get_node(c.node_id).await?;
            let mn = db.get_node(m.node_id).await?;
            if let (Some(cn), Some(mn)) = (cn, mn) {
                if cn.kind == NodeKind::Module && mn.kind == NodeKind::File {
                    result
                        .entry(cn.name.clone())
                        .or_default()
                        .push(mn.name.clone());
                }
            }
        }
    }
    Ok(result)
}

async fn load_import_relationships(
    db: &dyn HomerStore,
) -> crate::error::Result<(HashMap<String, Vec<String>>, HashMap<String, Vec<String>>)> {
    let edges = db.get_edges_by_kind(HyperedgeKind::Imports).await?;
    let mut from: HashMap<String, Vec<String>> = HashMap::new();
    let mut by: HashMap<String, Vec<String>> = HashMap::new();

    for edge in &edges {
        let src = edge.members.iter().find(|m| m.role == "source");
        let tgt = edge.members.iter().find(|m| m.role == "target");
        if let (Some(s), Some(t)) = (src, tgt) {
            let sn = db.get_node(s.node_id).await?.map(|n| n.name);
            let tn = db.get_node(t.node_id).await?.map(|n| n.name);
            if let (Some(sn), Some(tn)) = (sn, tn) {
                let sd = dir_of(&sn).to_string();
                let td = dir_of(&tn).to_string();
                if sd != td {
                    from.entry(sd.clone()).or_default().push(td.clone());
                    by.entry(td).or_default().push(sd);
                }
            }
        }
    }
    Ok((from, by))
}

async fn load_external_deps(
    db: &dyn HomerStore,
) -> crate::error::Result<HashMap<String, Vec<String>>> {
    let edges = db.get_edges_by_kind(HyperedgeKind::DependsOn).await?;
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &edges {
        let user = edge.members.iter().find(|m| m.role == "dependent");
        let dep = edge.members.iter().find(|m| m.role == "dependency");
        if let (Some(u), Some(d)) = (user, dep) {
            let un = db.get_node(u.node_id).await?;
            let dn = db.get_node(d.node_id).await?;
            if let (Some(un), Some(dn)) = (un, dn) {
                if dn.kind == NodeKind::ExternalDep {
                    result
                        .entry(un.name.clone())
                        .or_default()
                        .push(dn.name.clone());
                }
            }
        }
    }
    Ok(result)
}

async fn load_dir_entities(
    db: &dyn HomerStore,
    salience: &HashMap<NodeId, (f64, String)>,
) -> crate::error::Result<HashMap<String, Vec<EntityEntry>>> {
    let mut result: HashMap<String, Vec<EntityEntry>> = HashMap::new();

    for kind in [NodeKind::Function, NodeKind::Type] {
        let nodes = db
            .find_nodes(&NodeFilter {
                kind: Some(kind),
                ..Default::default()
            })
            .await?;
        for node in &nodes {
            let file = node
                .metadata
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let dir = dir_of(file).to_string();
            let sal = salience.get(&node.id).cloned();
            result
                .entry(dir)
                .or_default()
                .push((node.name.clone(), sal));
        }
    }
    Ok(result)
}

// ── Rendering ────────────────────────────────────────────────────────

async fn render_all_module_contexts(
    db: &dyn HomerStore,
    _config: &HomerConfig,
    repo_root: &Path,
) -> crate::error::Result<u32> {
    let modules = db
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Module),
            ..Default::default()
        })
        .await?;

    if modules.is_empty() {
        return Ok(0);
    }

    let data = load_module_data(db).await?;
    let mut count = 0u32;

    for module in &modules {
        let dir = &module.name;
        let has_files = data.module_files.get(dir).is_some_and(|v| !v.is_empty());
        let has_entities = data.dir_entities.get(dir).is_some_and(|v| !v.is_empty());

        if !has_files && !has_entities {
            continue;
        }

        let content = render_single_module(dir, &data);

        let output_path = repo_root.join(dir).join(".context.md");
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
            })?;
        }
        std::fs::write(&output_path, content)
            .map_err(|e| crate::error::HomerError::Extract(crate::error::ExtractError::Io(e)))?;
        count += 1;
    }

    Ok(count)
}

fn render_single_module(dir: &str, data: &ModuleData) -> String {
    let mut out = String::with_capacity(2048);

    let display_dir = if dir == "." { "(root)" } else { dir };
    writeln!(out, "# {display_dir}").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "> Auto-generated by Homer for `{dir}/`").unwrap();
    writeln!(out).unwrap();

    render_purpose(&mut out, dir, data);
    render_key_entities(&mut out, dir, data);
    render_dependencies(&mut out, dir, data);
    render_related_docs(&mut out, dir, data);
    render_change_profile(&mut out, dir, data);
    render_conventions(&mut out, dir, data);
    render_recent_changes(&mut out, dir, data);

    out
}

fn render_purpose(out: &mut String, dir: &str, data: &ModuleData) {
    // Look for a semantic summary of a file in this directory
    if let Some(files) = data.module_files.get(dir) {
        for file_name in files {
            if let Some(&fid) = data.file_ids.get(file_name) {
                if let Some(summary) = data.summaries.get(&fid) {
                    writeln!(out, "## Purpose").unwrap();
                    writeln!(out).unwrap();
                    writeln!(out, "{summary}").unwrap();
                    writeln!(out).unwrap();
                    return;
                }
            }
        }
    }

    // Fallback: derive purpose from entity names
    if let Some(ents) = data.dir_entities.get(dir) {
        if !ents.is_empty() {
            let top_names: Vec<_> = ents
                .iter()
                .take(5)
                .map(|(n, _)| n.rsplit("::").next().unwrap_or(n))
                .collect();
            writeln!(out, "## Purpose").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "Module containing: {}", top_names.join(", ")).unwrap();
            writeln!(out).unwrap();
        }
    }
}

fn render_key_entities(out: &mut String, dir: &str, data: &ModuleData) {
    let Some(ents) = data.dir_entities.get(dir) else {
        return;
    };
    if ents.is_empty() {
        return;
    }

    let mut sorted: Vec<_> = ents.iter().collect();
    sorted.sort_by(|a, b| {
        let sa = a.1.as_ref().map_or(0.0, |s| s.0);
        let sb = b.1.as_ref().map_or(0.0, |s| s.0);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    writeln!(out, "## Key Entities").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "| Name | Salience | Classification |").unwrap();
    writeln!(out, "|------|----------|----------------|").unwrap();

    for (name, salience) in sorted.iter().take(20) {
        let (score_str, cls_str) = match salience {
            Some((val, cls)) => (format!("{val:.2}"), cls.as_str()),
            None => ("\u{2014}".to_string(), "\u{2014}"),
        };
        let short_name = name.rsplit("::").next().unwrap_or(name);
        writeln!(out, "| `{short_name}` | {score_str} | {cls_str} |").unwrap();
    }
    writeln!(out).unwrap();
}

fn render_dependencies(out: &mut String, dir: &str, data: &ModuleData) {
    writeln!(out, "## Dependencies").unwrap();
    writeln!(out).unwrap();

    render_dep_list(out, "Imports from", data.imports_from.get(dir));
    render_dep_list(out, "Imported by", data.imported_by.get(dir));
    render_dep_list(out, "External", data.ext_deps.get(dir));

    writeln!(out).unwrap();
}

fn render_dep_list(out: &mut String, label: &str, items: Option<&Vec<String>>) {
    let Some(list) = items else { return };
    let mut unique: Vec<_> = list.iter().collect();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return;
    }

    write!(out, "- **{label}**: ").unwrap();
    let names: Vec<_> = unique.iter().map(|d| format!("`{d}`")).collect();
    writeln!(out, "{}", names.join(", ")).unwrap();
}

fn render_related_docs(out: &mut String, dir: &str, data: &ModuleData) {
    let Some(docs) = data.doc_refs.get(dir) else {
        return;
    };
    if docs.is_empty() {
        return;
    }

    writeln!(out, "## Related Documentation").unwrap();
    writeln!(out).unwrap();

    for doc in docs.iter().take(10) {
        writeln!(out, "- `{doc}`").unwrap();
    }
    writeln!(out).unwrap();
}

fn render_change_profile(out: &mut String, dir: &str, data: &ModuleData) {
    writeln!(out, "## Change Profile").unwrap();
    writeln!(out).unwrap();

    let Some(files) = data.module_files.get(dir) else {
        writeln!(out, "*No change data available.*").unwrap();
        writeln!(out).unwrap();
        return;
    };

    let mut total_changes = 0u64;
    let mut min_bf = u64::MAX;

    for file_name in files {
        if let Some(&fid) = data.file_ids.get(file_name) {
            if let Some(&freq) = data.freq.get(&fid) {
                total_changes += freq;
            }
            if let Some(&bf) = data.bus_factor.get(&fid) {
                min_bf = min_bf.min(bf);
            }
            if let Some(stab) = data.stability.get(&fid) {
                writeln!(out, "- **Stability**: {stab}").unwrap();
            }
        }
    }

    let file_count = files.len().max(1) as u64;
    if total_changes > 0 {
        let avg = total_changes / file_count;
        writeln!(out, "- **Total changes**: {total_changes} (avg {avg}/file)").unwrap();
    }
    if min_bf < u64::MAX {
        writeln!(out, "- **Min bus factor**: {min_bf}").unwrap();
    }

    let mut max_sal = 0.0_f64;
    for file_name in files {
        if let Some(&fid) = data.file_ids.get(file_name) {
            if let Some((s, _)) = data.salience.get(&fid) {
                max_sal = max_sal.max(*s);
            }
        }
    }
    if max_sal > 0.0 {
        writeln!(out, "- **Peak salience**: {max_sal:.2}").unwrap();
    }

    // Co-change modules
    if let Some(partners) = data.co_changes.get(dir) {
        if !partners.is_empty() {
            let display: Vec<_> = partners.iter().take(5).map(|p| format!("`{p}`")).collect();
            writeln!(out, "- **Co-changes with**: {}", display.join(", ")).unwrap();
        }
    }

    writeln!(out).unwrap();
}

fn render_conventions(out: &mut String, dir: &str, data: &ModuleData) {
    // Show project-wide naming convention as baseline
    let Some(convention) = &data.naming_convention else {
        return;
    };

    // Check if this directory has any entities that deviate
    let Some(ents) = data.dir_entities.get(dir) else {
        return;
    };
    if ents.is_empty() {
        return;
    }

    writeln!(out, "## Conventions").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "- **Naming**: {convention} (project-wide)").unwrap();
    writeln!(out).unwrap();
}

fn render_recent_changes(out: &mut String, dir: &str, data: &ModuleData) {
    let Some(commits) = data.recent_commits.get(dir) else {
        return;
    };
    if commits.is_empty() {
        return;
    }

    writeln!(out, "## Recent Significant Changes").unwrap();
    writeln!(out).unwrap();

    for (label, file) in commits.iter().take(5) {
        writeln!(out, "- {label} (`{file}`)").unwrap();
    }
    writeln!(out).unwrap();
}

fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or(".", |i| &path[..i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Hyperedge, HyperedgeId, HyperedgeMember, Node, NodeId};
    use chrono::Utc;

    async fn setup_module_data(db: &SqliteStore) {
        let now = Utc::now();

        db.upsert_node(&Node {
            id: NodeId(0),
            kind: NodeKind::Module,
            name: "src".to_string(),
            content_hash: None,
            last_extracted: now,
            metadata: HashMap::new(),
        })
        .await
        .unwrap();

        let file_a = db
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/main.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("language".to_string(), serde_json::json!("rust"));
                    m
                },
            })
            .await
            .unwrap();

        db.upsert_node(&Node {
            id: NodeId(0),
            kind: NodeKind::Function,
            name: "src/main.rs::main".to_string(),
            content_hash: None,
            last_extracted: now,
            metadata: {
                let mut m = HashMap::new();
                m.insert("file".to_string(), serde_json::json!("src/main.rs"));
                m
            },
        })
        .await
        .unwrap();

        let src_mod = db
            .get_node_by_name(NodeKind::Module, "src")
            .await
            .unwrap()
            .unwrap();

        db.upsert_hyperedge(&Hyperedge {
            id: HyperedgeId(0),
            kind: HyperedgeKind::BelongsTo,
            members: vec![
                HyperedgeMember {
                    node_id: file_a,
                    role: "member".to_string(),
                    position: 0,
                },
                HyperedgeMember {
                    node_id: src_mod.id,
                    role: "container".to_string(),
                    position: 1,
                },
            ],
            confidence: 1.0,
            last_updated: now,
            metadata: HashMap::new(),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn renders_module_context() {
        let store = SqliteStore::in_memory().unwrap();
        setup_module_data(&store).await;

        let tmp = tempfile::tempdir().unwrap();
        let config = HomerConfig::default();
        let renderer = ModuleContextRenderer;

        renderer.write(&store, &config, tmp.path()).await.unwrap();

        let ctx_path = tmp.path().join("src/.context.md");
        assert!(ctx_path.exists(), ".context.md should be created for src/");

        let content = std::fs::read_to_string(&ctx_path).unwrap();
        assert!(content.contains("# src"), "Should have module title");
        assert!(
            content.contains("## Purpose"),
            "Should have purpose section: {content}"
        );
        assert!(
            content.contains("## Key Entities"),
            "Should have entities section"
        );
        assert!(content.contains("main"), "Should list main function");
        assert!(
            content.contains("## Dependencies"),
            "Should have dependencies section"
        );
        assert!(
            content.contains("## Change Profile"),
            "Should have change profile"
        );
    }

    #[tokio::test]
    async fn skips_empty_modules() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: "empty".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let config = HomerConfig::default();
        let renderer = ModuleContextRenderer;

        renderer.write(&store, &config, tmp.path()).await.unwrap();

        let ctx_path = tmp.path().join("empty/.context.md");
        assert!(
            !ctx_path.exists(),
            "Should not create .context.md for empty module"
        );
    }

    #[test]
    fn dir_of_extracts_directory() {
        assert_eq!(dir_of("src/main.rs"), "src");
        assert_eq!(dir_of("lib.rs"), ".");
        assert_eq!(dir_of("a/b/c.rs"), "a/b");
    }
}
