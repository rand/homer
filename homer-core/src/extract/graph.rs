use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use rayon::prelude::*;
use tracing::{debug, info, instrument};

use homer_graphs::scope_graph::{FileScopeGraph, ScopeGraph, ScopeNodeId};
use homer_graphs::{
    HeuristicGraph, LanguageRegistry, ResolutionTier, SymbolKind as GraphSymbolKind,
    call_graph::{self, project_call_graph},
};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::traits::{ExtractStats, Extractor};

#[derive(Debug)]
pub struct GraphExtractor {
    repo_path: PathBuf,
    registry: LanguageRegistry,
}

impl GraphExtractor {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
            registry: LanguageRegistry::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Extractor for GraphExtractor {
    fn name(&self) -> &'static str {
        "graph"
    }

    async fn has_work(&self, store: &dyn HomerStore) -> crate::error::Result<bool> {
        let graph_sha = store.get_checkpoint("graph_last_sha").await?;
        let git_sha = store.get_checkpoint("git_last_sha").await?;
        // Re-run if graph checkpoint is missing or differs from current git checkpoint
        Ok(graph_sha != git_sha)
    }

    #[instrument(skip_all, name = "graph_extract")]
    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let start = Instant::now();
        let mut stats = ExtractStats::default();

        // Get all source files that have been stored by the structure extractor
        let file_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::File),
            ..Default::default()
        };
        let file_nodes = store.find_nodes(&file_filter).await?;
        info!(file_count = file_nodes.len(), "Graph extraction starting");

        for file_node in &file_nodes {
            let file_path = self.repo_path.join(&file_node.name);

            // Check if this file has a supported language
            let Some(lang) = self.registry.for_file(&file_path) else {
                continue;
            };

            // Check if this file's extension is included in configured patterns
            let file_ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_included = config
                .extraction
                .structure
                .include_patterns
                .iter()
                .any(|p| p.contains(&format!("*.{file_ext}")));
            if !is_included && !file_ext.is_empty() {
                // Also check if a language-supported extension is in the patterns
                let lang_included = lang.extensions().iter().any(|ext| {
                    config
                        .extraction
                        .structure
                        .include_patterns
                        .iter()
                        .any(|p| p.contains(&format!("*.{ext}")))
                });
                if !lang_included {
                    continue;
                }
            }

            match self
                .process_file(store, &mut stats, &file_path, file_node, lang.as_ref())
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    let path_str = file_node.name.clone();
                    debug!(path = %path_str, error = %e, "Failed to extract graph");
                    stats.errors.push((path_str, e));
                }
            }
        }

        // ── Scope graph resolution pass ──────────────────────────────
        // Build scope graphs for Precise-tier languages, resolve cross-file
        // references, and project high-confidence call edges.
        self.resolve_scope_graphs(store, &mut stats, &file_nodes, config)
            .await;

        // Save graph checkpoint using the git HEAD sha
        if let Ok(Some(git_sha)) = store.get_checkpoint("git_last_sha").await {
            store.set_checkpoint("graph_last_sha", &git_sha).await?;
        }

        stats.duration = start.elapsed();
        info!(
            nodes = stats.nodes_created,
            edges = stats.edges_created,
            errors = stats.errors.len(),
            duration = ?stats.duration,
            "Graph extraction complete"
        );
        Ok(stats)
    }
}

impl GraphExtractor {
    async fn process_file(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        file_path: &Path,
        file_node: &Node,
        lang: &dyn homer_graphs::LanguageSupport,
    ) -> crate::error::Result<()> {
        let source = std::fs::read_to_string(file_path)
            .map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;

        // Parse with tree-sitter
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&lang.tree_sitter_language())
            .map_err(|e| {
                HomerError::Extract(ExtractError::Parse {
                    path: file_path.to_string_lossy().to_string(),
                    message: format!("Failed to set language: {e}"),
                })
            })?;

        let tree = parser.parse(&source, None).ok_or_else(|| {
            HomerError::Extract(ExtractError::Parse {
                path: file_path.to_string_lossy().to_string(),
                message: "tree-sitter parse returned None".to_string(),
            })
        })?;

        // Run heuristic extraction
        let graph = lang
            .extract_heuristic(&tree, &source, file_path)
            .map_err(|e| {
                HomerError::Extract(ExtractError::Parse {
                    path: file_path.to_string_lossy().to_string(),
                    message: e.to_string(),
                })
            })?;

        // Store definitions, calls, and imports
        self.store_definitions(store, stats, &graph, file_node)
            .await?;
        self.store_calls(store, stats, &graph, file_node).await?;
        self.store_imports(store, stats, &graph, file_node).await?;

        Ok(())
    }

    async fn resolve_scope_graphs(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        file_nodes: &[Node],
        config: &HomerConfig,
    ) {
        let mut scope_graph = ScopeGraph::new();

        // Collect eligible files for scope graph building
        let eligible_files: Vec<(&Node, PathBuf)> = file_nodes
            .iter()
            .filter_map(|file_node| {
                let file_path = self.repo_path.join(&file_node.name);
                let lang = self.registry.for_file(&file_path)?;
                if lang.tier() != ResolutionTier::Precise {
                    return None;
                }
                let file_ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let lang_included = lang.extensions().iter().any(|ext| {
                    config
                        .extraction
                        .structure
                        .include_patterns
                        .iter()
                        .any(|p| p.contains(&format!("*.{ext}")))
                });
                if !lang_included && !file_ext.is_empty() {
                    return None;
                }
                Some((file_node, file_path))
            })
            .collect();

        // Parallel: read files, parse with tree-sitter, build scope graphs
        let file_scope_graphs: Vec<FileScopeGraph> = eligible_files
            .par_iter()
            .filter_map(|(_, file_path)| {
                let source = std::fs::read_to_string(file_path).ok()?;
                let lang = self.registry.for_file(file_path)?;
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&lang.tree_sitter_language()).ok()?;
                let tree = parser.parse(&source, None)?;
                lang.build_scope_graph(&tree, &source, file_path)
                    .ok()
                    .flatten()
            })
            .collect();

        // Merge all file scope graphs and compute enclosing functions
        let mut all_enclosing: HashMap<ScopeNodeId, ScopeNodeId> = HashMap::new();

        for fsg in &file_scope_graphs {
            let per_file_enclosing = call_graph::compute_enclosing_functions(fsg);
            let id_map = scope_graph.add_file_graph(fsg);

            // Remap enclosing function IDs to global IDs
            for (ref_id, func_id) in &per_file_enclosing {
                if let (Some(&new_ref), Some(&new_func)) = (id_map.get(ref_id), id_map.get(func_id))
                {
                    all_enclosing.insert(new_ref, new_func);
                }
            }
        }

        let resolved = scope_graph.resolve_all();
        if resolved.is_empty() {
            return;
        }

        let cg = project_call_graph(&scope_graph, &resolved, &all_enclosing);
        info!(
            resolved_refs = resolved.len(),
            call_edges = cg.edges.len(),
            "Scope graph resolution complete"
        );

        // Store high-confidence call edges from scope graph resolution
        for edge in &cg.edges {
            if let Err(e) = self.store_resolved_call(store, stats, edge).await {
                debug!(error = %e, "Failed to store resolved call edge");
            }
        }
    }

    async fn store_resolved_call(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        edge: &call_graph::CallEdge,
    ) -> crate::error::Result<()> {
        // Look up source (caller) and target (callee) by their scoped names
        let src_rel = edge
            .caller_file
            .strip_prefix(&self.repo_path)
            .unwrap_or(&edge.caller_file);
        let dst_rel = edge
            .callee_file
            .strip_prefix(&self.repo_path)
            .unwrap_or(&edge.callee_file);

        let src_scoped = format!("{}::{}", src_rel.display(), edge.caller_name);
        let dst_scoped = format!("{}::{}", dst_rel.display(), edge.callee_name);

        let src_node = store
            .get_node_by_name(NodeKind::Function, &src_scoped)
            .await?;
        let dst_node = store
            .get_node_by_name(NodeKind::Function, &dst_scoped)
            .await?;

        let (Some(src), Some(dst)) = (src_node, dst_node) else {
            return Ok(()); // One or both not found in store — skip
        };

        let mut metadata = HashMap::new();
        metadata.insert("resolution".to_string(), serde_json::json!("scope_graph"));
        if let Some(span) = edge.call_span {
            metadata.insert(
                "span".to_string(),
                serde_json::json!({
                    "start_row": span.start_row,
                    "start_col": span.start_col,
                }),
            );
        }

        store
            .upsert_hyperedge(&Hyperedge {
                id: HyperedgeId(0),
                kind: HyperedgeKind::Calls,
                members: vec![
                    HyperedgeMember {
                        node_id: src.id,
                        role: "caller".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: dst.id,
                        role: "callee".to_string(),
                        position: 1,
                    },
                ],
                confidence: edge.confidence,
                last_updated: Utc::now(),
                metadata,
            })
            .await?;
        stats.edges_created += 1;

        Ok(())
    }

    async fn store_definitions(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        graph: &HeuristicGraph,
        file_node: &Node,
    ) -> crate::error::Result<()> {
        for def in &graph.definitions {
            let node_kind = match def.kind {
                GraphSymbolKind::Function => NodeKind::Function,
                GraphSymbolKind::Type | GraphSymbolKind::Module => NodeKind::Type,
                // Variables, constants, and fields don't have dedicated NodeKind yet
                GraphSymbolKind::Variable | GraphSymbolKind::Constant | GraphSymbolKind::Field => {
                    continue;
                }
            };

            let mut metadata = HashMap::new();
            metadata.insert("file".to_string(), serde_json::json!(file_node.name));
            metadata.insert(
                "qualified_name".to_string(),
                serde_json::json!(def.qualified_name),
            );
            metadata.insert(
                "span".to_string(),
                serde_json::json!({
                    "start_row": def.span.start_row,
                    "start_col": def.span.start_col,
                    "end_row": def.span.end_row,
                    "end_col": def.span.end_col,
                }),
            );

            if let Some(doc) = &def.doc_comment {
                metadata.insert("doc_comment".to_string(), serde_json::json!(doc.text));
                metadata.insert(
                    "doc_style".to_string(),
                    serde_json::json!(format!("{:?}", doc.style)),
                );
            }

            // Use file-scoped qualified name to avoid collisions across files
            let scoped_name = format!("{}::{}", file_node.name, def.qualified_name);

            let def_node_id = store
                .upsert_node(&Node {
                    id: NodeId(0),
                    kind: node_kind,
                    name: scoped_name,
                    content_hash: None,
                    last_extracted: Utc::now(),
                    metadata,
                })
                .await?;
            stats.nodes_created += 1;

            // Create BelongsTo edge from definition → file
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::BelongsTo,
                    members: vec![
                        HyperedgeMember {
                            node_id: def_node_id,
                            role: "member".to_string(),
                            position: 0,
                        },
                        HyperedgeMember {
                            node_id: file_node.id,
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
        }

        Ok(())
    }

    async fn store_calls(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        graph: &HeuristicGraph,
        file_node: &Node,
    ) -> crate::error::Result<()> {
        for call in &graph.calls {
            // Look up caller node by scoped name
            let caller_scoped = format!("{}::{}", file_node.name, call.caller);
            let caller_node = store
                .get_node_by_name(NodeKind::Function, &caller_scoped)
                .await?;

            let caller_id = match caller_node {
                Some(n) => n.id,
                None => continue, // Caller not found, skip
            };

            let mut metadata = HashMap::new();
            metadata.insert(
                "callee_name".to_string(),
                serde_json::json!(call.callee_name),
            );
            metadata.insert(
                "span".to_string(),
                serde_json::json!({
                    "start_row": call.span.start_row,
                    "start_col": call.span.start_col,
                }),
            );

            // Try to find callee node (may not exist if external/unresolved)
            let target_scoped = format!("{}::{}", file_node.name, call.callee_name);
            let target_node = store
                .get_node_by_name(NodeKind::Function, &target_scoped)
                .await?;

            if let Some(target) = target_node {
                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Calls,
                        members: vec![
                            HyperedgeMember {
                                node_id: caller_id,
                                role: "caller".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: target.id,
                                role: "callee".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: call.confidence,
                        last_updated: Utc::now(),
                        metadata,
                    })
                    .await?;
                stats.edges_created += 1;
            }
            // If callee not found, we still record it as metadata on the caller
            // but don't create an edge (unresolved reference)
        }

        Ok(())
    }

    async fn store_imports(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        graph: &HeuristicGraph,
        file_node: &Node,
    ) -> crate::error::Result<()> {
        for import in &graph.imports {
            let mut metadata = HashMap::new();
            metadata.insert(
                "imported_name".to_string(),
                serde_json::json!(import.imported_name),
            );
            if let Some(target) = &import.target_path {
                metadata.insert(
                    "target_path".to_string(),
                    serde_json::json!(target.to_string_lossy()),
                );
            }

            // Resolve import target to a file node for proper directed edges.
            // Try target_path first, then fall back to heuristic name matching.
            let target_node_id = self.resolve_import_target(store, import, file_node).await;

            let members = if let Some(target_id) = target_node_id {
                // Skip self-imports
                if target_id == file_node.id {
                    continue;
                }
                vec![
                    HyperedgeMember {
                        node_id: file_node.id,
                        role: "importer".to_string(),
                        position: 0,
                    },
                    HyperedgeMember {
                        node_id: target_id,
                        role: "imported".to_string(),
                        position: 1,
                    },
                ]
            } else {
                // Unresolved import — store as single-member edge with metadata
                vec![HyperedgeMember {
                    node_id: file_node.id,
                    role: "importer".to_string(),
                    position: 0,
                }]
            };

            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Imports,
                    members,
                    confidence: import.confidence,
                    last_updated: Utc::now(),
                    metadata,
                })
                .await?;
            stats.edges_created += 1;
        }

        Ok(())
    }

    /// Resolve an import to a target file `NodeId` in the store.
    async fn resolve_import_target(
        &self,
        store: &dyn HomerStore,
        import: &homer_graphs::HeuristicImport,
        file_node: &Node,
    ) -> Option<NodeId> {
        // 1. Try explicit target_path from the heuristic extractor
        if let Some(target_path) = &import.target_path {
            let rel = target_path
                .strip_prefix(&self.repo_path)
                .unwrap_or(target_path);
            let rel_str = rel.to_string_lossy();
            if let Ok(Some(node)) = store.get_node_by_name(NodeKind::File, &rel_str).await {
                return Some(node.id);
            }
        }

        let import_name = &import.imported_name;

        // 2. Rust crate-relative imports: crate::module::path::Type → module/path.rs
        if let Some(path) = import_name.strip_prefix("crate::") {
            return self.resolve_rust_crate_import(store, path, file_node).await;
        }

        // 3. Rust super imports: super::foo → parent module
        if import_name.starts_with("super::") {
            return self
                .resolve_rust_super_import(store, import_name, file_node)
                .await;
        }

        // 4. Python/JS style: dotted or slash paths
        let last_component = import_name
            .rsplit("::")
            .next()
            .or_else(|| import_name.rsplit('.').next())
            .or_else(|| import_name.rsplit('/').next())
            .unwrap_or(import_name);

        if last_component.is_empty() || last_component == "*" {
            return None;
        }

        // Search for files whose name contains this component
        let file_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::File),
            name_contains: Some(last_component.to_string()),
            ..Default::default()
        };

        if let Ok(matches) = store.find_nodes(&file_filter).await {
            if matches.len() == 1 {
                return Some(matches[0].id);
            }
        }

        None
    }

    /// Resolve Rust `crate::` import paths to file nodes.
    /// `crate::store::HomerStore` → look for `<crate>/src/store/mod.rs` or `<crate>/src/store.rs`
    async fn resolve_rust_crate_import(
        &self,
        store: &dyn HomerStore,
        path: &str,
        file_node: &Node,
    ) -> Option<NodeId> {
        // Split path into module segments (drop the final type/item name if PascalCase)
        let segments: Vec<&str> = path.split("::").collect();
        if segments.is_empty() {
            return None;
        }

        // Determine the crate root from the file's path (e.g., "homer-core/src/...")
        let file_name = &file_node.name;
        let crate_prefix = file_name.find("/src/").map(|i| &file_name[..i])?;

        // Try progressively fewer segments (last segment might be a type name, not a module)
        for take in (1..=segments.len()).rev() {
            let module_path = segments[..take].join("/");

            // Try: crate_prefix/src/module_path.rs
            let candidate = format!("{crate_prefix}/src/{module_path}.rs");
            if let Ok(Some(node)) = store.get_node_by_name(NodeKind::File, &candidate).await {
                return Some(node.id);
            }

            // Try: crate_prefix/src/module_path/mod.rs
            let candidate_mod = format!("{crate_prefix}/src/{module_path}/mod.rs");
            if let Ok(Some(node)) = store.get_node_by_name(NodeKind::File, &candidate_mod).await {
                return Some(node.id);
            }
        }

        None
    }

    /// Resolve Rust `super::` imports relative to the current file's parent.
    async fn resolve_rust_super_import(
        &self,
        store: &dyn HomerStore,
        import_name: &str,
        file_node: &Node,
    ) -> Option<NodeId> {
        let file_name = &file_node.name;
        // Get parent directory
        let parent = std::path::Path::new(file_name).parent()?;

        // Strip "super::" prefix(es) and walk up
        let mut current = parent.to_path_buf();
        let mut rest = import_name;
        while let Some(stripped) = rest.strip_prefix("super::") {
            current = current.parent()?.to_path_buf();
            rest = stripped;
        }

        // rest is now the remaining module path (or "*")
        if rest == "*" {
            // super::* → parent mod.rs
            let mod_path = current.join("mod.rs");
            let mod_str = mod_path.to_string_lossy();
            if let Ok(Some(node)) = store.get_node_by_name(NodeKind::File, &mod_str).await {
                return Some(node.id);
            }
            return None;
        }

        // Try rest as a file
        let segments: Vec<&str> = rest.split("::").collect();
        for take in (1..=segments.len()).rev() {
            let module_path = segments[..take].join("/");
            let candidate = current.join(format!("{module_path}.rs"));
            let candidate_str = candidate.to_string_lossy();
            if let Ok(Some(node)) = store.get_node_by_name(NodeKind::File, &candidate_str).await {
                return Some(node.id);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    fn create_test_project(dir: &Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();

        // Rust source file
        std::fs::write(
            dir.join("src/main.rs"),
            "/// Entry point.\nfn main() {\n    greet();\n}\n\n/// Say hello.\nfn greet() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        // Python source file
        std::fs::write(
            dir.join("src/utils.py"),
            "def helper():\n    \"\"\"Help function.\"\"\"\n    pass\n\ndef run():\n    helper()\n",
        )
        .unwrap();

        // TypeScript source file
        std::fs::write(
            dir.join("src/app.ts"),
            "function start(): void {\n    console.log('start');\n}\n\nclass App {\n    run() { start(); }\n}\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn extract_graph_from_files() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();

        // First run structure extractor to create File nodes
        let struct_ext = crate::extract::structure::StructureExtractor::new(tmp.path());
        struct_ext.extract(&store, &config).await.unwrap();

        // Run graph extractor
        let graph_ext = GraphExtractor::new(tmp.path());
        let stats = graph_ext.extract(&store, &config).await.unwrap();

        assert!(
            stats.nodes_created > 0,
            "Should create definition nodes, got {}",
            stats.nodes_created,
        );

        // Verify Function nodes exist
        let fn_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Function),
            ..Default::default()
        };
        let functions = store.find_nodes(&fn_filter).await.unwrap();
        assert!(
            functions.len() >= 4,
            "Should have main, greet, helper, run, start, App.run — got {}",
            functions.len()
        );

        // Verify Type nodes exist (App class)
        let type_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Type),
            ..Default::default()
        };
        let types = store.find_nodes(&type_filter).await.unwrap();
        assert!(!types.is_empty(), "Should have at least App type");

        // Verify doc comment metadata on greet
        let greet = functions
            .iter()
            .find(|f| f.name.contains("greet") && !f.name.contains("App"))
            .expect("Should find greet function");
        assert!(
            greet.metadata.contains_key("doc_comment"),
            "greet should have doc_comment metadata"
        );

        // Verify edges created (BelongsTo + Imports at minimum)
        assert!(stats.edges_created > 0, "Should create edges");
    }
}
