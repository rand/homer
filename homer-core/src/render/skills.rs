// Skills renderer — produces Claude Code skill files in `.claude/skills/`.
//
// Derives skills from two sources:
// 1. Task patterns (from prompt analysis) — higher confidence
// 2. Co-change patterns (from commit analysis) — lower confidence

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;

use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, HyperedgeKind, NodeFilter, NodeKind};

use super::traits::Renderer;

#[derive(Debug)]
pub struct SkillsRenderer;

#[async_trait::async_trait]
impl Renderer for SkillsRenderer {
    fn name(&self) -> &'static str {
        "skills"
    }

    fn output_path(&self) -> &'static str {
        ".claude/skills/homer-skills.md"
    }

    #[instrument(skip_all, name = "skills_render")]
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
        let skills = derive_skills(store, config).await?;

        if skills.is_empty() {
            info!("No skills derived — skipping skills output");
            return Ok(());
        }

        let skills_dir = repo_root.join(".claude/skills");
        std::fs::create_dir_all(&skills_dir)
            .map_err(|e| crate::error::HomerError::Extract(crate::error::ExtractError::Io(e)))?;

        let mut written = 0u32;
        for skill in &skills {
            let filename = skill_filename(&skill.name);
            let path = skills_dir.join(&filename);
            let content = render_skill(skill);
            std::fs::write(&path, content).map_err(|e| {
                crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
            })?;
            written += 1;
        }

        info!(skills = written, "Skills rendered");
        Ok(())
    }
}

// ── Skill data model ─────────────────────────────────────────────────

struct DerivedSkill {
    name: String,
    description: String,
    source: SkillSource,
    files: Vec<String>,
    frequency: u32,
    patterns: Vec<String>,
    pitfalls: Vec<String>,
}

enum SkillSource {
    TaskPattern,
    CoChange,
}

// ── Skill derivation ─────────────────────────────────────────────────

async fn derive_skills(
    store: &dyn HomerStore,
    _config: &HomerConfig,
) -> crate::error::Result<Vec<DerivedSkill>> {
    let mut skills = Vec::new();

    // Source 1: Task patterns from prompt analysis
    derive_from_task_patterns(store, &mut skills).await?;

    // Source 2: Co-change patterns from commit analysis
    derive_from_co_changes(store, &mut skills).await?;

    // Sort by frequency (most common first)
    skills.sort_by(|a, b| b.frequency.cmp(&a.frequency));

    Ok(skills)
}

async fn derive_from_task_patterns(
    store: &dyn HomerStore,
    skills: &mut Vec<DerivedSkill>,
) -> crate::error::Result<()> {
    let mod_filter = NodeFilter {
        kind: Some(NodeKind::Module),
        ..Default::default()
    };
    let modules = store.find_nodes(&mod_filter).await?;
    let root_id = modules.iter().min_by_key(|m| m.name.len()).map(|m| m.id);

    let Some(root_id) = root_id else {
        return Ok(());
    };

    let Some(result) = store
        .get_analysis(root_id, AnalysisKind::TaskPattern)
        .await?
    else {
        return Ok(());
    };

    let Some(patterns) = result.data.get("patterns").and_then(|v| v.as_array()) else {
        return Ok(());
    };

    for pattern in patterns {
        let name = pattern
            .get("pattern_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        #[allow(clippy::cast_possible_truncation)]
        let frequency = pattern
            .get("frequency")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let files: Vec<String> = pattern
            .get("typical_files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if frequency >= 2 && !files.is_empty() {
            skills.push(DerivedSkill {
                name: name.to_string(),
                description: format!("When performing: {name}"),
                source: SkillSource::TaskPattern,
                files,
                frequency,
                patterns: Vec::new(),
                pitfalls: Vec::new(),
            });
        }
    }

    // Add pitfalls from correction hotspots
    let corrections = store
        .get_analyses_by_kind(AnalysisKind::CorrectionHotspot)
        .await?;
    for correction in &corrections {
        let is_confusion = correction
            .data
            .get("is_confusion_zone")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !is_confusion {
            continue;
        }

        let file_name = store
            .get_node(correction.node_id)
            .await?
            .map_or_else(String::new, |n| n.name);
        if file_name.is_empty() {
            continue;
        }

        // Add pitfall to any skill that touches this file
        for skill in skills.iter_mut() {
            if skill.files.iter().any(|f| f == &file_name) {
                let rate = correction
                    .data
                    .get("correction_rate")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                skill.pitfalls.push(format!(
                    "`{file_name}` has a {:.0}% correction rate — proceed carefully",
                    rate * 100.0
                ));
            }
        }
    }

    Ok(())
}

async fn derive_from_co_changes(
    store: &dyn HomerStore,
    skills: &mut Vec<DerivedSkill>,
) -> crate::error::Result<()> {
    let co_change_edges = store.get_edges_by_kind(HyperedgeKind::CoChanges).await?;

    for edge in &co_change_edges {
        #[allow(clippy::cast_possible_truncation)]
        let arity = edge
            .metadata
            .get("arity")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let co_occurrences = edge
            .metadata
            .get("co_occurrences")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;

        // Only derive skills from groups of 3+ files with sufficient frequency
        if arity < 3 || co_occurrences < 3 {
            continue;
        }

        let mut files = Vec::new();
        for member in &edge.members {
            let name = store
                .get_node(member.node_id)
                .await?
                .map_or_else(String::new, |n| n.name);
            if !name.is_empty() {
                files.push(name);
            }
        }

        if files.is_empty() {
            continue;
        }

        // Check if this file set is already covered by a task pattern skill
        let already_covered = skills
            .iter()
            .any(|s| files.iter().filter(|f| s.files.contains(f)).count() >= files.len() / 2);
        if already_covered {
            continue;
        }

        // Derive a name from the common directory
        let common_dir = common_prefix(&files);
        let skill_name = format!("Modify {common_dir} files");

        skills.push(DerivedSkill {
            name: skill_name,
            description: format!(
                "When modifying files in `{common_dir}` — these {} files change together",
                files.len()
            ),
            source: SkillSource::CoChange,
            files,
            frequency: co_occurrences,
            patterns: Vec::new(),
            pitfalls: Vec::new(),
        });
    }

    Ok(())
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_skill(skill: &DerivedSkill) -> String {
    let mut out = String::with_capacity(1024);

    // Frontmatter
    writeln!(out, "---").unwrap();
    writeln!(out, "description: \"{}\"", skill.description).unwrap();
    writeln!(out, "---").unwrap();
    writeln!(out).unwrap();

    // Title
    writeln!(out, "# {}", skill.name).unwrap();
    writeln!(out).unwrap();

    let source_label = match skill.source {
        SkillSource::TaskPattern => "agent interaction patterns",
        SkillSource::CoChange => "co-change analysis",
    };
    writeln!(
        out,
        "Based on analysis of {} occurrences from {source_label}:",
        skill.frequency
    )
    .unwrap();
    writeln!(out).unwrap();

    // Files
    writeln!(out, "## Files to modify").unwrap();
    writeln!(out).unwrap();
    for (i, file) in skill.files.iter().enumerate() {
        writeln!(out, "{}. `{file}`", i + 1).unwrap();
    }
    writeln!(out).unwrap();

    // Patterns
    if !skill.patterns.is_empty() {
        writeln!(out, "## Patterns to follow").unwrap();
        writeln!(out).unwrap();
        for pattern in &skill.patterns {
            writeln!(out, "- {pattern}").unwrap();
        }
        writeln!(out).unwrap();
    }

    // Pitfalls
    if !skill.pitfalls.is_empty() {
        writeln!(out, "## Common pitfalls").unwrap();
        writeln!(out).unwrap();
        for pitfall in &skill.pitfalls {
            writeln!(out, "- {pitfall}").unwrap();
        }
        writeln!(out).unwrap();
    }

    out
}

fn skill_filename(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse multiple hyphens
    let mut result = String::new();
    let mut last_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_hyphen {
                result.push(c);
            }
            last_hyphen = true;
        } else {
            result.push(c);
            last_hyphen = false;
        }
    }
    let trimmed = result.trim_matches('-');
    format!("homer-{trimmed}.md")
}

fn common_prefix(files: &[String]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let dirs: Vec<&str> = files
        .iter()
        .filter_map(|f| f.rfind('/').map(|i| &f[..i]))
        .collect();

    if dirs.is_empty() {
        return ".".to_string();
    }

    // Find most common directory
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for dir in &dirs {
        *counts.entry(dir).or_default() += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map_or_else(|| ".".to_string(), |(d, _)| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{
        AnalysisResult, AnalysisResultId, Hyperedge, HyperedgeId, HyperedgeMember, Node, NodeId,
    };
    use chrono::Utc;

    #[tokio::test]
    async fn derives_skills_from_task_patterns() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        // Create root module
        let root = store
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

        // Store task pattern data
        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: root,
                kind: AnalysisKind::TaskPattern,
                data: serde_json::json!({
                    "total_sessions": 20,
                    "patterns": [
                        {
                            "pattern_name": "Add API endpoint",
                            "frequency": 8,
                            "typical_files": ["src/routes.rs", "src/handlers.rs", "tests/api_test.rs"]
                        },
                        {
                            "pattern_name": "Fix auth bug",
                            "frequency": 3,
                            "typical_files": ["src/auth.rs", "src/middleware.rs"]
                        }
                    ]
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        let config = HomerConfig::default();
        let skills = derive_skills(&store, &config).await.unwrap();

        assert_eq!(skills.len(), 2, "Should derive 2 skills");
        assert_eq!(skills[0].name, "Add API endpoint");
        assert_eq!(skills[0].frequency, 8);
        assert_eq!(skills[0].files.len(), 3);
        assert_eq!(skills[1].name, "Fix auth bug");
    }

    #[tokio::test]
    async fn derives_skills_from_co_changes() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        // Create file nodes
        let file_a = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/model.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let file_b = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/schema.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        let file_c = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/migration.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        // Create a CoChanges hyperedge (arity 3)
        let mut meta = HashMap::new();
        meta.insert("arity".to_string(), serde_json::json!(3));
        meta.insert("co_occurrences".to_string(), serde_json::json!(5));
        meta.insert("support".to_string(), serde_json::json!(0.6));

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::CoChanges,
                members: vec![
                    HyperedgeMember {
                        node_id: file_a,
                        role: "file".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: file_b,
                        role: "file".to_string(),
                        position: 1,
                    },
                    HyperedgeMember {
                        node_id: file_c,
                        role: "file".to_string(),
                        position: 2,
                    },
                ],
                confidence: 0.8,
                last_updated: now,
                metadata: meta,
            })
            .await
            .unwrap();

        let config = HomerConfig::default();
        let skills = derive_skills(&store, &config).await.unwrap();

        assert_eq!(skills.len(), 1, "Should derive 1 skill from co-changes");
        assert_eq!(skills[0].files.len(), 3);
        assert!(skills[0].name.contains("src"));
    }

    #[tokio::test]
    async fn skill_rendering_format() {
        let skill = DerivedSkill {
            name: "Add API endpoint".to_string(),
            description: "When adding a new API endpoint".to_string(),
            source: SkillSource::TaskPattern,
            files: vec!["src/routes.rs".to_string(), "src/handlers.rs".to_string()],
            frequency: 8,
            patterns: vec!["Follow RESTful naming conventions".to_string()],
            pitfalls: vec!["`src/routes.rs` has a 30% correction rate".to_string()],
        };

        let content = render_skill(&skill);

        assert!(content.contains("---\n"), "Should have frontmatter");
        assert!(content.contains("description:"), "Should have description");
        assert!(content.contains("# Add API endpoint"), "Should have title");
        assert!(content.contains("## Files to modify"), "Should list files");
        assert!(content.contains("src/routes.rs"), "Should include file");
        assert!(
            content.contains("## Patterns to follow"),
            "Should have patterns"
        );
        assert!(
            content.contains("## Common pitfalls"),
            "Should have pitfalls"
        );
    }

    #[test]
    fn skill_filename_generation() {
        assert_eq!(
            skill_filename("Add API endpoint"),
            "homer-add-api-endpoint.md"
        );
        assert_eq!(
            skill_filename("Fix auth/session bug"),
            "homer-fix-auth-session-bug.md"
        );
    }

    #[test]
    fn common_prefix_finds_directory() {
        let files = vec![
            "src/api/routes.rs".to_string(),
            "src/api/handlers.rs".to_string(),
            "src/api/tests.rs".to_string(),
        ];
        assert_eq!(common_prefix(&files), "src/api");
    }
}
