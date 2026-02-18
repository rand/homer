use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use tracing::{debug, info, instrument, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::git::ExtractStats;

/// Structure extractor — file tree walking, manifest parsing, CI config detection.
#[derive(Debug)]
pub struct StructureExtractor {
    repo_path: PathBuf,
}

impl StructureExtractor {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }

    /// Run structure extraction.
    #[instrument(skip_all, name = "structure_extract")]
    pub async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        // Create root Module node
        let root_name = self
            .repo_path
            .file_name()
            .map_or_else(|| "root".to_string(), |n| n.to_string_lossy().to_string());

        let root_module_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: root_name,
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.nodes_created += 1;

        // Walk file tree
        let files = self.walk_file_tree(config);
        info!(file_count = files.len(), "Structure scan found files");

        // Track directories seen for Module nodes
        let mut dir_modules: HashMap<PathBuf, NodeId> = HashMap::new();

        for file_path in &files {
            match self
                .process_file(
                    store,
                    config,
                    &mut stats,
                    file_path,
                    root_module_id,
                    &mut dir_modules,
                )
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    let path_str = file_path.to_string_lossy().to_string();
                    warn!(path = %path_str, error = %e, "Failed to process file");
                    stats.errors.push((path_str, e));
                }
            }
        }

        // Parse manifests
        self.extract_manifests(store, &mut stats, root_module_id)
            .await?;

        // Extract CI config metadata
        self.extract_ci_config(store, root_module_id).await?;

        stats.duration = start.elapsed();
        info!(
            nodes = stats.nodes_created,
            edges = stats.edges_created,
            errors = stats.errors.len(),
            duration = ?stats.duration,
            "Structure extraction complete"
        );
        Ok(stats)
    }

    fn walk_file_tree(&self, config: &HomerConfig) -> Vec<PathBuf> {
        let structure = &config.extraction.structure;
        let mut matched_files = Vec::new();

        // Build include matchers
        for pattern in &structure.include_patterns {
            let full_pattern = self.repo_path.join(pattern).to_string_lossy().to_string();
            match glob::glob(&full_pattern) {
                Ok(paths) => {
                    for entry in paths.flatten() {
                        if entry.is_file()
                            && !is_excluded(&entry, &self.repo_path, &structure.exclude_patterns)
                        {
                            matched_files.push(entry);
                        }
                    }
                }
                Err(e) => {
                    warn!(pattern = %pattern, error = %e, "Invalid glob pattern");
                }
            }
        }

        // Deduplicate
        matched_files.sort();
        matched_files.dedup();

        matched_files
    }

    async fn process_file(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
        stats: &mut ExtractStats,
        file_path: &Path,
        root_module_id: NodeId,
        dir_modules: &mut HashMap<PathBuf, NodeId>,
    ) -> crate::error::Result<()> {
        let relative = file_path.strip_prefix(&self.repo_path).unwrap_or(file_path);

        // Compute content hash
        let content =
            std::fs::read(file_path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;
        let content_hash = hash_bytes(&content);

        // Detect language from extension
        let language = detect_language(file_path);

        let mut metadata = HashMap::new();
        if let Some(lang) = &language {
            metadata.insert("language".to_string(), serde_json::json!(lang));
        }
        metadata.insert("size_bytes".to_string(), serde_json::json!(content.len()));

        // Create File node
        let file_node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: relative.to_string_lossy().to_string(),
                content_hash: Some(content_hash),
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;
        stats.nodes_created += 1;

        // Create Module node for parent directory + BelongsTo edge
        let module_id = if let Some(parent) = relative.parent() {
            if parent.as_os_str().is_empty() {
                root_module_id
            } else {
                self.ensure_module(store, stats, parent, root_module_id, dir_modules)
                    .await?
            }
        } else {
            root_module_id
        };

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::BelongsTo,
                members: vec![
                    HyperedgeMember {
                        node_id: file_node_id,
                        role: "member".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: module_id,
                        role: "container".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.edges_created += 1;

        // Index file content for FTS (first ~1000 bytes for searchability)
        let text_preview = String::from_utf8_lossy(&content);
        let preview: &str = if text_preview.len() > 1000 {
            // Find the nearest char boundary at or before byte 1000
            let mut end = 1000;
            while !text_preview.is_char_boundary(end) {
                end -= 1;
            }
            &text_preview[..end]
        } else {
            &text_preview
        };
        store
            .index_text(file_node_id, "source_code", preview)
            .await?;

        Ok(())
    }

    async fn ensure_module(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        dir_path: &Path,
        root_module_id: NodeId,
        dir_modules: &mut HashMap<PathBuf, NodeId>,
    ) -> crate::error::Result<NodeId> {
        if let Some(&id) = dir_modules.get(dir_path) {
            return Ok(id);
        }

        let module_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: dir_path.to_string_lossy().to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.nodes_created += 1;

        // Create BelongsTo edge to parent module
        let parent_id = if let Some(parent) = dir_path.parent() {
            if parent.as_os_str().is_empty() {
                root_module_id
            } else {
                Box::pin(self.ensure_module(store, stats, parent, root_module_id, dir_modules))
                    .await?
            }
        } else {
            root_module_id
        };

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::BelongsTo,
                members: vec![
                    HyperedgeMember {
                        node_id: module_id,
                        role: "member".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: parent_id,
                        role: "container".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.edges_created += 1;

        dir_modules.insert(dir_path.to_path_buf(), module_id);
        Ok(module_id)
    }

    async fn extract_manifests(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let mut build_systems: Vec<String> = Vec::new();

        // Cargo.toml
        let cargo_toml = self.repo_path.join("Cargo.toml");
        if cargo_toml.exists() {
            build_systems.push("cargo".to_string());
            if let Err(e) = self
                .parse_cargo_toml(store, stats, &cargo_toml, root_module_id)
                .await
            {
                warn!(error = %e, "Failed to parse Cargo.toml");
            }
        }

        // package.json
        let package_json = self.repo_path.join("package.json");
        if package_json.exists() {
            build_systems.push("npm".to_string());
            if let Err(e) = self
                .parse_package_json(store, stats, &package_json, root_module_id)
                .await
            {
                warn!(error = %e, "Failed to parse package.json");
            }
        }

        // pyproject.toml
        let pyproject = self.repo_path.join("pyproject.toml");
        if pyproject.exists() {
            build_systems.push("python".to_string());
            if let Err(e) = self
                .parse_pyproject_toml(store, stats, &pyproject, root_module_id)
                .await
            {
                warn!(error = %e, "Failed to parse pyproject.toml");
            }
        }

        // go.mod
        let gomod = self.repo_path.join("go.mod");
        if gomod.exists() {
            build_systems.push("go".to_string());
            if let Err(e) = self
                .parse_go_mod(store, stats, &gomod, root_module_id)
                .await
            {
                warn!(error = %e, "Failed to parse go.mod");
            }
        }

        // Store detected build systems on the root module
        if !build_systems.is_empty() {
            if let Some(mut root) = store.get_node(root_module_id).await? {
                root.metadata.insert(
                    "build_systems".to_string(),
                    serde_json::json!(build_systems),
                );
                store.upsert_node(&root).await?;
            }
        }

        Ok(())
    }

    async fn parse_cargo_toml(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        path: &Path,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;
        let table: toml::Table = content.parse().map_err(|e: toml::de::Error| {
            HomerError::Extract(ExtractError::Parse {
                path: path.to_string_lossy().to_string(),
                message: e.to_string(),
            })
        })?;

        let deps_sections = ["dependencies", "dev-dependencies", "build-dependencies"];
        for section_name in deps_sections {
            if let Some(deps) = table.get(section_name).and_then(|v| v.as_table()) {
                let dev_only = section_name != "dependencies";
                for (name, value) in deps {
                    let version = extract_cargo_version(value);
                    self.store_dependency(
                        store,
                        stats,
                        name,
                        version.as_deref(),
                        dev_only,
                        root_module_id,
                    )
                    .await?;
                }
            }
        }

        // Also check workspace dependencies
        if let Some(workspace) = table.get("workspace").and_then(|v| v.as_table()) {
            if let Some(deps) = workspace.get("dependencies").and_then(|v| v.as_table()) {
                for (name, value) in deps {
                    let version = extract_cargo_version(value);
                    self.store_dependency(
                        store,
                        stats,
                        name,
                        version.as_deref(),
                        false,
                        root_module_id,
                    )
                    .await?;
                }
            }
        }

        debug!(path = %path.display(), "Parsed Cargo.toml");
        Ok(())
    }

    async fn parse_package_json(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        path: &Path,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;
        let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            HomerError::Extract(ExtractError::Parse {
                path: path.to_string_lossy().to_string(),
                message: e.to_string(),
            })
        })?;

        for (section, dev_only) in [("dependencies", false), ("devDependencies", true)] {
            if let Some(deps) = json.get(section).and_then(|v| v.as_object()) {
                for (name, version) in deps {
                    let ver = version.as_str().map(String::from);
                    self.store_dependency(
                        store,
                        stats,
                        name,
                        ver.as_deref(),
                        dev_only,
                        root_module_id,
                    )
                    .await?;
                }
            }
        }

        debug!(path = %path.display(), "Parsed package.json");
        Ok(())
    }

    async fn parse_pyproject_toml(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        path: &Path,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;
        let table: toml::Table = content.parse().map_err(|e: toml::de::Error| {
            HomerError::Extract(ExtractError::Parse {
                path: path.to_string_lossy().to_string(),
                message: e.to_string(),
            })
        })?;

        // PEP 621 dependencies
        if let Some(project) = table.get("project").and_then(|v| v.as_table()) {
            if let Some(deps) = project.get("dependencies").and_then(|v| v.as_array()) {
                for dep in deps {
                    if let Some(dep_str) = dep.as_str() {
                        let name = dep_str
                            .split(['>', '<', '=', '~', '!', ';', '['])
                            .next()
                            .unwrap_or(dep_str)
                            .trim();
                        self.store_dependency(store, stats, name, None, false, root_module_id)
                            .await?;
                    }
                }
            }
        }

        debug!(path = %path.display(), "Parsed pyproject.toml");
        Ok(())
    }

    async fn parse_go_mod(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        path: &Path,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let content =
            std::fs::read_to_string(path).map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;

        // Simple line-based parsing of go.mod require blocks
        let mut in_require = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("require (") || trimmed == "require (" {
                in_require = true;
                continue;
            }
            if trimmed == ")" {
                in_require = false;
                continue;
            }
            if in_require || trimmed.starts_with("require ") {
                let dep_line = if trimmed.starts_with("require ") {
                    trimmed.strip_prefix("require ").unwrap_or(trimmed)
                } else {
                    trimmed
                };
                let parts: Vec<&str> = dep_line.split_whitespace().collect();
                if let Some(name) = parts.first() {
                    let version = parts.get(1).copied();
                    self.store_dependency(store, stats, name, version, false, root_module_id)
                        .await?;
                }
            }
        }

        debug!(path = %path.display(), "Parsed go.mod");
        Ok(())
    }

    async fn store_dependency(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        name: &str,
        version: Option<&str>,
        dev_only: bool,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let mut metadata = HashMap::new();
        if let Some(v) = version {
            metadata.insert("version".to_string(), serde_json::json!(v));
        }
        metadata.insert("dev_only".to_string(), serde_json::json!(dev_only));

        let dep_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::ExternalDep,
                name: name.to_string(),
                content_hash: None,
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;
        stats.nodes_created += 1;

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::DependsOn,
                members: vec![
                    HyperedgeMember {
                        node_id: root_module_id,
                        role: "dependent".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: dep_id,
                        role: "dependency".to_string(),
                        position: 1,
                    },
                ],
                confidence: 1.0,
                last_updated: Utc::now(),
                metadata: HashMap::new(),
            })
            .await?;
        stats.edges_created += 1;

        Ok(())
    }

    async fn extract_ci_config(
        &self,
        store: &dyn HomerStore,
        root_module_id: NodeId,
    ) -> crate::error::Result<()> {
        let mut ci_commands: HashMap<String, Vec<String>> = HashMap::new();

        // GitHub Actions
        for workflow_glob in &[
            self.repo_path.join(".github/workflows/*.yml"),
            self.repo_path.join(".github/workflows/*.yaml"),
        ] {
            let pattern = workflow_glob.to_string_lossy().to_string();
            if let Ok(paths) = glob::glob(&pattern) {
                for entry in paths.flatten() {
                    if let Ok(content) = std::fs::read_to_string(&entry) {
                        extract_yaml_run_commands(&content, &mut ci_commands);
                    }
                }
            }
        }

        // Makefile
        let makefile = self.repo_path.join("Makefile");
        if makefile.exists() {
            if let Ok(content) = std::fs::read_to_string(&makefile) {
                extract_makefile_targets(&content, &mut ci_commands);
            }
        }

        // package.json scripts
        let package_json = self.repo_path.join("package.json");
        if package_json.exists() {
            if let Ok(content) = std::fs::read_to_string(&package_json) {
                extract_npm_scripts(&content, &mut ci_commands);
            }
        }

        if !ci_commands.is_empty() {
            // Update root module metadata with CI info
            if let Some(mut root) = store.get_node(root_module_id).await? {
                root.metadata
                    .insert("ci_commands".to_string(), serde_json::json!(ci_commands));
                store.upsert_node(&root).await?;
            }
        }

        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn is_excluded(path: &Path, repo_root: &Path, exclude_patterns: &[String]) -> bool {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    let rel_str = relative.to_string_lossy();

    for pattern in exclude_patterns {
        // Simple check: if pattern ends with /**, check prefix
        let normalized = pattern.replace("**", "");
        let normalized = normalized.trim_matches('/');
        if rel_str.contains(normalized) {
            return true;
        }
    }
    false
}

fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let lang = match ext {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cxx" | "cc" | "hpp" => "cpp",
        "rb" => "ruby",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "zig" => "zig",
        _ => return None,
    };
    Some(lang.to_string())
}

fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

fn extract_cargo_version(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(t) => t.get("version").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    }
}

fn extract_yaml_run_commands(content: &str, commands: &mut HashMap<String, Vec<String>>) {
    // Simple heuristic: find `run:` lines in YAML
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(cmd) = trimmed.strip_prefix("run:") {
            let cmd = cmd.trim().to_string();
            if !cmd.is_empty() {
                commands
                    .entry("github_actions".to_string())
                    .or_default()
                    .push(cmd);
            }
        }
    }
}

fn extract_makefile_targets(content: &str, commands: &mut HashMap<String, Vec<String>>) {
    for line in content.lines() {
        // Match target lines like "test:" or "build:"
        if let Some(target) = line.strip_suffix(':') {
            let target = target.trim();
            if !target.is_empty()
                && !target.starts_with('#')
                && !target.starts_with('.')
                && !target.contains(' ')
            {
                commands
                    .entry("makefile".to_string())
                    .or_default()
                    .push(target.to_string());
            }
        }
    }
}

fn extract_npm_scripts(content: &str, commands: &mut HashMap<String, Vec<String>>) {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
            for (name, value) in scripts {
                if let Some(cmd) = value.as_str() {
                    commands
                        .entry("npm_scripts".to_string())
                        .or_default()
                        .push(format!("{name}: {cmd}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    fn create_test_project(dir: &Path) {
        // Create directory structure
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();

        // Source files
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();
        std::fs::write(dir.join("tests/test_main.rs"), "#[test] fn it_works() {}").unwrap();

        // Cargo.toml
        std::fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
tempfile = "3"
"#,
        )
        .unwrap();

        // README
        std::fs::write(dir.join("README.md"), "# Test Project\nA test project.").unwrap();

        // Makefile
        std::fs::write(
            dir.join("Makefile"),
            "test:\n\tcargo test\n\nbuild:\n\tcargo build\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn extract_file_tree() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = StructureExtractor::new(tmp.path());

        let stats = extractor.extract(&store, &config).await.unwrap();

        assert!(stats.nodes_created > 0, "Should create nodes");
        assert!(stats.edges_created > 0, "Should create edges");

        // Verify File nodes
        let file_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::File),
            ..Default::default()
        };
        let files = store.find_nodes(&file_filter).await.unwrap();
        assert!(
            files.len() >= 3,
            "Should have at least 3 source files, got {}",
            files.len()
        );

        // Verify Module nodes
        let mod_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Module),
            ..Default::default()
        };
        let modules = store.find_nodes(&mod_filter).await.unwrap();
        assert!(
            modules.len() >= 2,
            "Should have root + src modules, got {}",
            modules.len()
        );

        // Verify ExternalDep nodes
        let dep_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::ExternalDep),
            ..Default::default()
        };
        let deps = store.find_nodes(&dep_filter).await.unwrap();
        assert!(
            deps.len() >= 3,
            "Should have serde, tokio, tempfile deps, got {}",
            deps.len()
        );

        // Verify content hash is set on files
        let first_file = &files[0];
        assert!(
            first_file.content_hash.is_some(),
            "File should have content hash"
        );
    }

    #[tokio::test]
    async fn extract_ci_config() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let extractor = StructureExtractor::new(tmp.path());

        extractor.extract(&store, &config).await.unwrap();

        // Root module should have CI commands
        let mod_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Module),
            ..Default::default()
        };
        let modules = store.find_nodes(&mod_filter).await.unwrap();
        let root = modules
            .iter()
            .find(|m| m.metadata.contains_key("ci_commands"))
            .expect("Root module should have ci_commands");

        let ci = root.metadata.get("ci_commands").unwrap();
        assert!(
            ci.get("makefile").is_some(),
            "Should detect Makefile targets"
        );
    }

    #[test]
    fn detect_language_by_extension() {
        assert_eq!(
            detect_language(Path::new("foo.rs")),
            Some("rust".to_string())
        );
        assert_eq!(
            detect_language(Path::new("bar.py")),
            Some("python".to_string())
        );
        assert_eq!(
            detect_language(Path::new("baz.ts")),
            Some("typescript".to_string())
        );
        assert_eq!(detect_language(Path::new("qux.go")), Some("go".to_string()));
        assert_eq!(detect_language(Path::new("readme.md")), None);
    }
}
