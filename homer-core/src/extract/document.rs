use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use tracing::{info, warn};

use crate::config::HomerConfig;
use crate::error::{ExtractError, HomerError};
use crate::store::HomerStore;
use crate::types::{
    DocumentType, Hyperedge, HyperedgeId, HyperedgeKind, HyperedgeMember, Node, NodeId, NodeKind,
};

use super::traits::ExtractStats;

#[derive(Debug)]
pub struct DocumentExtractor {
    repo_path: PathBuf,
}

impl DocumentExtractor {
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

        if !config.extraction.documents.enabled {
            info!("Document extraction disabled");
            return Ok(stats);
        }

        let doc_files = self.find_document_files(config);
        info!(file_count = doc_files.len(), "Found document files");

        for doc_path in &doc_files {
            match self.process_document(store, &mut stats, doc_path).await {
                Ok(()) => {}
                Err(e) => {
                    let path_str = doc_path.to_string_lossy().to_string();
                    warn!(path = %path_str, error = %e, "Failed to process document");
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
            "Document extraction complete"
        );
        Ok(stats)
    }

    fn find_document_files(&self, config: &HomerConfig) -> Vec<PathBuf> {
        let doc_config = &config.extraction.documents;
        let mut matched = Vec::new();

        for pattern in &doc_config.include_patterns {
            let full_pattern = self.repo_path.join(pattern).to_string_lossy().to_string();
            match glob::glob(&full_pattern) {
                Ok(paths) => {
                    for entry in paths.flatten() {
                        if entry.is_file()
                            && !is_excluded(&entry, &self.repo_path, &doc_config.exclude_patterns)
                        {
                            matched.push(entry);
                        }
                    }
                }
                Err(e) => {
                    warn!(pattern = %pattern, error = %e, "Invalid glob pattern");
                }
            }
        }

        matched.sort();
        matched.dedup();
        matched
    }

    async fn process_document(
        &self,
        store: &dyn HomerStore,
        stats: &mut ExtractStats,
        doc_path: &Path,
    ) -> crate::error::Result<()> {
        let content = std::fs::read_to_string(doc_path)
            .map_err(|e| HomerError::Extract(ExtractError::Io(e)))?;

        let relative = doc_path.strip_prefix(&self.repo_path).unwrap_or(doc_path);

        let doc_type = classify_document(relative);
        let title = extract_title(&content, relative);
        let sections = extract_sections(&content);
        let word_count = content.split_whitespace().count();
        let content_hash = hash_str(&content);

        let mut metadata = HashMap::new();
        metadata.insert(
            "doc_type".to_string(),
            serde_json::json!(format!("{doc_type:?}")),
        );
        metadata.insert("title".to_string(), serde_json::json!(title));
        metadata.insert("sections".to_string(), serde_json::json!(sections));
        metadata.insert("word_count".to_string(), serde_json::json!(word_count));

        // Check for homer:preserve markers (AGENTS.md circularity)
        let has_preserve_markers = content.contains("<!-- homer:preserve -->");
        if has_preserve_markers {
            metadata.insert("has_preserve_markers".to_string(), serde_json::json!(true));
        }

        let doc_node_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Document,
                name: relative.to_string_lossy().to_string(),
                content_hash: Some(content_hash),
                last_extracted: Utc::now(),
                metadata,
            })
            .await?;
        stats.nodes_created += 1;

        // Extract cross-references and create Documents edges
        let refs = extract_cross_references(&content, &self.repo_path);
        for xref in &refs {
            // Try to find the referenced File node
            if let Ok(Some(target_node)) = store
                .get_node_by_name(NodeKind::File, &xref.target_path)
                .await
            {
                let mut edge_metadata = HashMap::new();
                edge_metadata.insert(
                    "ref_type".to_string(),
                    serde_json::json!(format!("{:?}", xref.ref_type)),
                );

                store
                    .upsert_hyperedge(&Hyperedge {
                        id: HyperedgeId(0),
                        kind: HyperedgeKind::Documents,
                        members: vec![
                            HyperedgeMember {
                                node_id: doc_node_id,
                                role: "document".to_string(),
                                position: 0,
                            },
                            HyperedgeMember {
                                node_id: target_node.id,
                                role: "subject".to_string(),
                                position: 1,
                            },
                        ],
                        confidence: xref.confidence,
                        last_updated: Utc::now(),
                        metadata: edge_metadata,
                    })
                    .await?;
                stats.edges_created += 1;
            }
        }

        // Index document content for FTS
        let preview = if content.len() > 2000 {
            &content[..2000]
        } else {
            &content
        };
        store.index_text(doc_node_id, "document", preview).await?;

        Ok(())
    }
}

// ── Cross-reference types ─────────────────────────────────────────

#[derive(Debug)]
struct CrossReference {
    target_path: String,
    ref_type: RefType,
    confidence: f64,
}

#[derive(Debug)]
enum RefType {
    Link,
    BacktickPath,
    PathMention,
}

// ── Markdown parsing helpers ──────────────────────────────────────

fn classify_document(path: &Path) -> DocumentType {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_default();

    if name.starts_with("README") {
        DocumentType::Readme
    } else if name.starts_with("CONTRIBUTING") {
        DocumentType::Contributing
    } else if name.starts_with("ARCHITECTURE") || name.starts_with("DESIGN") {
        DocumentType::Architecture
    } else if name.starts_with("CHANGELOG") || name.starts_with("CHANGES") {
        DocumentType::Changelog
    } else if name.starts_with("AGENTS") {
        DocumentType::Runbook
    } else {
        // Check path components for ADR
        let path_str = path.to_string_lossy().to_lowercase();
        if path_str.contains("adr/") || path_str.contains("adr\\") {
            DocumentType::Adr
        } else if path_str.contains("doc/") || path_str.contains("docs/") {
            DocumentType::Guide
        } else {
            DocumentType::Other
        }
    }
}

fn extract_title(content: &str, path: &Path) -> String {
    // Try to find a level-1 heading
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            return heading.trim().to_string();
        }
    }
    // Fallback to filename
    path.file_stem().map_or_else(
        || "Untitled".to_string(),
        |s| s.to_string_lossy().to_string(),
    )
}

fn extract_sections(content: &str) -> Vec<String> {
    let mut sections = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            sections.push(heading.trim().to_string());
        } else if let Some(heading) = trimmed.strip_prefix("### ") {
            sections.push(heading.trim().to_string());
        }
    }
    sections
}

fn extract_cross_references(content: &str, repo_root: &Path) -> Vec<CrossReference> {
    let mut refs = Vec::new();

    for line in content.lines() {
        // Markdown links: [text](path)
        extract_link_refs(line, repo_root, &mut refs);

        // Backtick paths: `src/foo.rs`
        extract_backtick_refs(line, repo_root, &mut refs);

        // Bare path mentions: src/foo.rs (word boundary)
        extract_path_mentions(line, repo_root, &mut refs);
    }

    // Deduplicate by target_path
    refs.sort_by(|a, b| a.target_path.cmp(&b.target_path));
    refs.dedup_by(|a, b| a.target_path == b.target_path);
    refs
}

fn extract_link_refs(line: &str, repo_root: &Path, refs: &mut Vec<CrossReference>) {
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find(')') {
            let link_target = &after[..end];
            // Skip external URLs and anchors
            if !link_target.starts_with("http")
                && !link_target.starts_with('#')
                && !link_target.starts_with("mailto:")
            {
                // Strip anchor fragments
                let path_part = link_target.split('#').next().unwrap_or(link_target);
                if !path_part.is_empty() && repo_root.join(path_part).exists() {
                    refs.push(CrossReference {
                        target_path: normalize_path(path_part),
                        ref_type: RefType::Link,
                        confidence: 0.95,
                    });
                }
            }
            rest = &after[end..];
        } else {
            break;
        }
    }
}

fn extract_backtick_refs(line: &str, repo_root: &Path, refs: &mut Vec<CrossReference>) {
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        if let Some(end) = after.find('`') {
            let inside = &after[..end];
            // Check if it looks like a file path (has extension or slash)
            if looks_like_path(inside) && repo_root.join(inside).exists() {
                refs.push(CrossReference {
                    target_path: normalize_path(inside),
                    ref_type: RefType::BacktickPath,
                    confidence: 0.85,
                });
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
}

fn extract_path_mentions(line: &str, repo_root: &Path, refs: &mut Vec<CrossReference>) {
    for word in line.split_whitespace() {
        // Strip common punctuation
        let cleaned = word.trim_matches(|c: char| c == ',' || c == '.' || c == ':' || c == ';');
        if looks_like_path(cleaned) && !cleaned.starts_with("http") && !cleaned.starts_with('#') {
            // Only if the path actually exists on disk
            if repo_root.join(cleaned).exists() {
                refs.push(CrossReference {
                    target_path: normalize_path(cleaned),
                    ref_type: RefType::PathMention,
                    confidence: 0.7,
                });
            }
        }
    }
}

fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() < 3 {
        return false;
    }
    // Must contain a slash or a file extension
    (s.contains('/') || s.contains('.'))
        && !s.starts_with("http")
        && !s.starts_with("mailto:")
        // Filter common false positives
        && !s.contains("e.g")
        && !s.contains("i.e")
}

fn normalize_path(path: &str) -> String {
    // Remove leading ./ and normalize separators
    let cleaned = path.strip_prefix("./").unwrap_or(path);
    cleaned.replace('\\', "/")
}

fn is_excluded(path: &Path, repo_root: &Path, exclude_patterns: &[String]) -> bool {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    let rel_str = relative.to_string_lossy();

    for pattern in exclude_patterns {
        let normalized = pattern.replace("**", "");
        let normalized = normalized.trim_matches('/');
        if !normalized.is_empty() && rel_str.contains(normalized) {
            return true;
        }
    }
    false
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    fn create_test_project(dir: &Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::create_dir_all(dir.join("docs/adr")).unwrap();

        // Source files (needed for cross-reference resolution)
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        // README with cross-references
        std::fs::write(
            dir.join("README.md"),
            "# My Project\n\n\
             ## Overview\n\n\
             This is a test project. See [the library](src/lib.rs) for the API.\n\n\
             ## Getting Started\n\n\
             Run `src/main.rs` to start.\n\n\
             ## Architecture\n\n\
             The main entry point is src/main.rs which calls functions from src/lib.rs.\n",
        )
        .unwrap();

        // ADR document
        std::fs::write(
            dir.join("docs/adr/001-use-rust.md"),
            "# ADR 001: Use Rust\n\n\
             ## Status\n\n\
             Accepted\n\n\
             ## Context\n\n\
             We need a systems language. See `src/lib.rs` for the core implementation.\n\n\
             ## Decision\n\n\
             Use Rust for safety and performance.\n",
        )
        .unwrap();

        // AGENTS.md with preserve markers
        std::fs::write(
            dir.join("AGENTS.md"),
            "# AGENTS.md\n\n\
             <!-- homer:preserve -->\n\
             ## Custom Section\n\
             Human-written content here.\n\
             <!-- /homer:preserve -->\n\n\
             ## Module Map\n\
             Auto-generated content.\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn extract_documents() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        // First run structure extractor so File nodes exist for cross-references
        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let struct_ext = crate::extract::structure::StructureExtractor::new(tmp.path());
        struct_ext.extract(&store, &config).await.unwrap();

        // Now run document extractor
        let doc_ext = DocumentExtractor::new(tmp.path());
        let stats = doc_ext.extract(&store, &config).await.unwrap();

        assert!(stats.nodes_created > 0, "Should create document nodes");

        // Verify Document nodes
        let doc_filter = crate::types::NodeFilter {
            kind: Some(NodeKind::Document),
            ..Default::default()
        };
        let docs = store.find_nodes(&doc_filter).await.unwrap();
        assert!(
            docs.len() >= 2,
            "Should have at least README + ADR, got {}",
            docs.len()
        );

        // Verify README metadata
        let readme = docs.iter().find(|d| d.name.contains("README")).unwrap();
        let title = readme.metadata.get("title").unwrap();
        assert_eq!(title, &serde_json::json!("My Project"));

        let sections = readme.metadata.get("sections").unwrap();
        let section_list: Vec<String> = serde_json::from_value(sections.clone()).unwrap();
        assert!(section_list.contains(&"Overview".to_string()));
        assert!(section_list.contains(&"Getting Started".to_string()));

        // Verify Documents edges (cross-references from README → source files)
        assert!(
            stats.edges_created > 0,
            "Should create cross-reference edges"
        );

        // Verify AGENTS.md has preserve marker metadata
        let agents = docs.iter().find(|d| d.name.contains("AGENTS"));
        if let Some(agents_doc) = agents {
            let has_markers = agents_doc
                .metadata
                .get("has_preserve_markers")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            assert!(has_markers, "AGENTS.md should have preserve markers");
        }
    }

    #[test]
    fn classify_document_types() {
        assert_eq!(
            classify_document(Path::new("README.md")),
            DocumentType::Readme
        );
        assert_eq!(
            classify_document(Path::new("CONTRIBUTING.md")),
            DocumentType::Contributing
        );
        assert_eq!(
            classify_document(Path::new("docs/adr/001-foo.md")),
            DocumentType::Adr
        );
        assert_eq!(
            classify_document(Path::new("CHANGELOG.md")),
            DocumentType::Changelog
        );
        assert_eq!(
            classify_document(Path::new("docs/guide.md")),
            DocumentType::Guide
        );
    }

    #[test]
    fn extract_title_from_markdown() {
        assert_eq!(
            extract_title("# My Title\n\nContent", Path::new("test.md")),
            "My Title"
        );
        assert_eq!(
            extract_title("No heading here", Path::new("test.md")),
            "test"
        );
    }

    #[test]
    fn extract_sections_from_markdown() {
        let content = "# Title\n## Section 1\nContent\n## Section 2\n### Sub\n";
        let sections = extract_sections(content);
        assert_eq!(sections, vec!["Section 1", "Section 2", "Sub"]);
    }

    #[test]
    fn cross_reference_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let content = "See [main](src/main.rs) and `src/main.rs` for details.";
        let refs = extract_cross_references(content, tmp.path());

        // Should find the reference (deduplicated)
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target_path, "src/main.rs");
    }
}
