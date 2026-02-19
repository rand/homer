#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, NodeFilter, NodeKind};

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Start reference (tag, branch, SHA)
    pub ref1: String,
    /// End reference (tag, branch, SHA, or HEAD)
    pub ref2: String,
    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json", "markdown"])]
    pub format: String,
    /// Path to git repository (default: current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
    /// Sections to include: topology, centrality, communities, coupling
    #[arg(long, value_delimiter = ',')]
    pub include: Option<Vec<String>>,
}

pub async fn run(args: DiffArgs) -> anyhow::Result<()> {
    let repo_path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot resolve path: {}", args.path.display()))?;

    let homer_dir = repo_path.join(".homer");
    if !homer_dir.exists() {
        anyhow::bail!(
            "Homer is not initialized in {}. Run `homer init` first.",
            repo_path.display()
        );
    }

    let db_path = super::resolve_db_path(&repo_path);
    let store = SqliteStore::open(&db_path)
        .with_context(|| format!("Cannot open database: {}", db_path.display()))?;

    let changed_files = git_diff_files(&repo_path, &args.ref1, &args.ref2)?;
    let impact = assess_impact(&store, &changed_files).await?;

    let sections = args
        .include
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect::<Vec<_>>());
    let filter = SectionFilter::new(sections.as_deref());

    match args.format.as_str() {
        "json" => print_json(&args.ref1, &args.ref2, &changed_files, &impact, &filter)?,
        "markdown" => {
            print_markdown(&args.ref1, &args.ref2, &changed_files, &impact, &filter);
        }
        _ => print_text(&args.ref1, &args.ref2, &changed_files, &impact, &filter),
    }

    Ok(())
}

// ── Git diff ──────────────────────────────────────────────────────

struct ChangedFile {
    path: String,
    status: FileStatus,
}

#[derive(Clone, Copy)]
enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileStatus {
    fn symbol(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Modified => "~",
            Self::Deleted => "-",
            Self::Renamed => ">",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
        }
    }
}

fn git_diff_files(
    repo_path: &std::path::Path,
    ref1: &str,
    ref2: &str,
) -> anyhow::Result<Vec<ChangedFile>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-status", ref1, ref2])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status = match parts[0].chars().next() {
            Some('A') => FileStatus::Added,
            Some('D') => FileStatus::Deleted,
            Some('R') => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        files.push(ChangedFile {
            path: parts[1].to_string(),
            status,
        });
    }

    Ok(files)
}

// ── Impact assessment ──────────────────────────────────────────────

struct ImpactReport {
    high_salience_touched: Vec<(String, f64, String)>,
    low_bus_factor_touched: Vec<(String, u64)>,
    modules_affected: Vec<String>,
    communities_affected: Vec<String>,
    topology: Topology,
}

struct Topology {
    added: u32,
    modified: u32,
    deleted: u32,
    renamed: u32,
}

async fn assess_impact(
    db: &dyn HomerStore,
    changed_files: &[ChangedFile],
) -> anyhow::Result<ImpactReport> {
    let topology = count_topology(changed_files);

    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let all_files = db.find_nodes(&file_filter).await?;
    let file_id_map: HashMap<&str, homer_core::types::NodeId> =
        all_files.iter().map(|f| (f.name.as_str(), f.id)).collect();
    let changed_paths: Vec<&str> = changed_files.iter().map(|f| f.path.as_str()).collect();

    let high_salience_touched = find_salience_impacts(db, &file_id_map, &changed_paths).await?;
    let low_bus_factor_touched = find_bus_factor_risks(db, &file_id_map, &changed_paths).await?;
    let communities_affected = find_community_impacts(db, &file_id_map, &changed_paths).await?;

    let mut modules: Vec<String> = changed_paths
        .iter()
        .filter_map(|p| p.rfind('/').map(|i| p[..i].to_string()))
        .collect();
    modules.sort();
    modules.dedup();

    Ok(ImpactReport {
        high_salience_touched,
        low_bus_factor_touched,
        modules_affected: modules,
        communities_affected,
        topology,
    })
}

fn count_topology(changed: &[ChangedFile]) -> Topology {
    Topology {
        added: changed
            .iter()
            .filter(|f| matches!(f.status, FileStatus::Added))
            .count() as u32,
        modified: changed
            .iter()
            .filter(|f| matches!(f.status, FileStatus::Modified))
            .count() as u32,
        deleted: changed
            .iter()
            .filter(|f| matches!(f.status, FileStatus::Deleted))
            .count() as u32,
        renamed: changed
            .iter()
            .filter(|f| matches!(f.status, FileStatus::Renamed))
            .count() as u32,
    }
}

async fn find_salience_impacts(
    db: &dyn HomerStore,
    file_ids: &HashMap<&str, homer_core::types::NodeId>,
    paths: &[&str],
) -> anyhow::Result<Vec<(String, f64, String)>> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await
        .unwrap_or_default();
    let map: HashMap<homer_core::types::NodeId, (f64, String)> = results
        .iter()
        .filter_map(|r| {
            let val = r.data.get("score")?.as_f64()?;
            let cls = r
                .data
                .get("classification")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            Some((r.node_id, (val, cls)))
        })
        .collect();

    let mut touched = Vec::new();
    for &path in paths {
        if let Some(&nid) = file_ids.get(path) {
            if let Some((val, cls)) = map.get(&nid) {
                if *val > 0.3 {
                    touched.push((path.to_string(), *val, cls.clone()));
                }
            }
        }
    }
    touched.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(touched)
}

async fn find_bus_factor_risks(
    db: &dyn HomerStore,
    file_ids: &HashMap<&str, homer_core::types::NodeId>,
    paths: &[&str],
) -> anyhow::Result<Vec<(String, u64)>> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await
        .unwrap_or_default();
    let map: HashMap<homer_core::types::NodeId, u64> = results
        .iter()
        .filter_map(|r| Some((r.node_id, r.data.get("bus_factor")?.as_u64()?)))
        .collect();

    let mut risky = Vec::new();
    for &path in paths {
        if let Some(&nid) = file_ids.get(path) {
            if let Some(&bf) = map.get(&nid) {
                if bf <= 1 {
                    risky.push((path.to_string(), bf));
                }
            }
        }
    }
    Ok(risky)
}

async fn find_community_impacts(
    db: &dyn HomerStore,
    file_ids: &HashMap<&str, homer_core::types::NodeId>,
    paths: &[&str],
) -> anyhow::Result<Vec<String>> {
    let results = db
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await
        .unwrap_or_default();
    let mut affected = Vec::new();
    for &path in paths {
        if let Some(&nid) = file_ids.get(path) {
            for r in &results {
                if r.node_id == nid {
                    if let Some(comm) = r
                        .data
                        .get("community_label")
                        .and_then(serde_json::Value::as_str)
                    {
                        affected.push(comm.to_string());
                    }
                }
            }
        }
    }
    affected.sort();
    affected.dedup();
    Ok(affected)
}

// ── Section filter ────────────────────────────────────────────────

#[allow(clippy::struct_excessive_bools)]
struct SectionFilter {
    show_topology: bool,
    show_centrality: bool,
    show_communities: bool,
    show_coupling: bool,
}

impl SectionFilter {
    fn new(sections: Option<&[&str]>) -> Self {
        let Some(sections) = sections else {
            return Self {
                show_topology: true,
                show_centrality: true,
                show_communities: true,
                show_coupling: true,
            };
        };
        Self {
            show_topology: sections.contains(&"topology"),
            show_centrality: sections.contains(&"centrality"),
            show_communities: sections.contains(&"communities"),
            show_coupling: sections.contains(&"coupling"),
        }
    }
}

// ── Output formatters ──────────────────────────────────────────────

fn print_text(
    ref1: &str,
    ref2: &str,
    changed: &[ChangedFile],
    impact: &ImpactReport,
    filter: &SectionFilter,
) {
    println!("Architectural Diff: {ref1} -> {ref2}");
    println!();

    if filter.show_topology {
        let t = &impact.topology;
        println!("Topology:");
        println!(
            "  +{} new, ~{} modified, -{} removed, >{} renamed",
            t.added, t.modified, t.deleted, t.renamed
        );
        println!("  {} files changed total", changed.len());
        println!();

        if !changed.is_empty() {
            println!("Files:");
            for f in changed.iter().take(30) {
                println!("  {} {}", f.status.symbol(), f.path);
            }
            if changed.len() > 30 {
                println!("  ... and {} more", changed.len() - 30);
            }
            println!();
        }
    }

    if filter.show_centrality && !impact.high_salience_touched.is_empty() {
        println!("High-Salience Files Touched:");
        for (name, sal, cls) in &impact.high_salience_touched {
            println!("  {name} (salience: {sal:.2}, {cls})");
        }
        println!();
    }

    if filter.show_coupling && !impact.low_bus_factor_touched.is_empty() {
        println!("Risk — Low Bus Factor:");
        for (name, bf) in &impact.low_bus_factor_touched {
            println!("  {name} (bus factor: {bf})");
        }
        println!();
    }

    if filter.show_coupling && !impact.modules_affected.is_empty() {
        println!("Modules Affected: {}", impact.modules_affected.join(", "));
        println!();
    }

    if filter.show_communities && !impact.communities_affected.is_empty() {
        println!(
            "Communities Affected: {}",
            impact.communities_affected.join(", ")
        );
    }
}

fn print_markdown(
    ref1: &str,
    ref2: &str,
    changed: &[ChangedFile],
    impact: &ImpactReport,
    filter: &SectionFilter,
) {
    println!("# Architectural Diff: {ref1} -> {ref2}");
    println!();

    if filter.show_topology {
        let t = &impact.topology;
        println!("## Topology");
        println!();
        println!(
            "- **+{}** new, **~{}** modified, **-{}** removed, **>{}** renamed",
            t.added, t.modified, t.deleted, t.renamed
        );
        println!("- **{}** files changed total", changed.len());
        println!();

        if !changed.is_empty() {
            println!("## Changed Files");
            println!();
            println!("| Status | File |");
            println!("|--------|------|");
            for f in changed.iter().take(50) {
                println!("| {} | `{}` |", f.status.label(), f.path);
            }
            if changed.len() > 50 {
                println!("| | ... and {} more |", changed.len() - 50);
            }
            println!();
        }
    }

    if filter.show_centrality && !impact.high_salience_touched.is_empty() {
        println!("## High-Salience Files Touched");
        println!();
        println!("| File | Salience | Classification |");
        println!("|------|----------|----------------|");
        for (name, sal, cls) in &impact.high_salience_touched {
            println!("| `{name}` | {sal:.2} | {cls} |");
        }
        println!();
    }

    if filter.show_coupling && !impact.low_bus_factor_touched.is_empty() {
        println!("## Risk: Low Bus Factor");
        println!();
        for (name, bf) in &impact.low_bus_factor_touched {
            println!("- `{name}` (bus factor: {bf})");
        }
        println!();
    }

    if filter.show_coupling && !impact.modules_affected.is_empty() {
        println!(
            "**Modules:** {}",
            impact
                .modules_affected
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
    }
}

fn print_json(
    ref1: &str,
    ref2: &str,
    changed: &[ChangedFile],
    impact: &ImpactReport,
    filter: &SectionFilter,
) -> anyhow::Result<()> {
    let mut json = serde_json::json!({
        "ref1": ref1,
        "ref2": ref2,
    });
    let obj = json.as_object_mut().unwrap();

    if filter.show_topology {
        let t = &impact.topology;
        obj.insert(
            "topology".into(),
            serde_json::json!({
                "added": t.added,
                "modified": t.modified,
                "deleted": t.deleted,
                "renamed": t.renamed,
                "total_changed": changed.len(),
            }),
        );
        obj.insert(
            "changed_files".into(),
            serde_json::json!(
                changed
                    .iter()
                    .map(|f| serde_json::json!({
                        "path": f.path,
                        "status": f.status.label(),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }

    if filter.show_centrality {
        obj.insert(
            "high_salience_touched".into(),
            serde_json::json!(
                impact
                    .high_salience_touched
                    .iter()
                    .map(|(name, sal, cls)| {
                        serde_json::json!({ "file": name, "salience": sal, "classification": cls })
                    })
                    .collect::<Vec<_>>()
            ),
        );
    }

    if filter.show_coupling {
        obj.insert(
            "low_bus_factor".into(),
            serde_json::json!(
                impact
                    .low_bus_factor_touched
                    .iter()
                    .map(|(name, bf)| { serde_json::json!({ "file": name, "bus_factor": bf }) })
                    .collect::<Vec<_>>()
            ),
        );
        obj.insert(
            "modules_affected".into(),
            serde_json::json!(impact.modules_affected),
        );
    }

    if filter.show_communities {
        obj.insert(
            "communities_affected".into(),
            serde_json::json!(impact.communities_affected),
        );
    }

    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
