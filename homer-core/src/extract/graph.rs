use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use tracing::{debug, info};

use homer_graphs::{HeuristicGraph, LanguageRegistry, SymbolKind as GraphSymbolKind};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::traits::ExtractStats;

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

    pub async fn extract(
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

            // Check language is enabled in config
            if !config
                .extraction
                .structure
                .include_patterns
                .iter()
                .any(|p| {
                    lang.extensions()
                        .iter()
                        .any(|ext| p.contains(&format!("*.{ext}")))
                })
            {
                continue;
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

            // Create Imports edge from file → import target (if resolvable)
            // For now, store as a self-referencing edge on the file with import metadata
            store
                .upsert_hyperedge(&Hyperedge {
                    id: HyperedgeId(0),
                    kind: HyperedgeKind::Imports,
                    members: vec![HyperedgeMember {
                        node_id: file_node.id,
                        role: "importer".to_string(),
                        position: 0,
                    }],
                    confidence: import.confidence,
                    last_updated: Utc::now(),
                    metadata,
                })
                .await?;
            stats.edges_created += 1;
        }

        Ok(())
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
