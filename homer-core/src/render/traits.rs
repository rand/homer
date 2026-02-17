use std::path::Path;

use crate::config::HomerConfig;
use crate::store::HomerStore;

/// Common interface for output artifact generators.
#[async_trait::async_trait]
pub trait Renderer: Send + Sync {
    /// Human-readable name for this renderer.
    fn name(&self) -> &'static str;

    /// Output file path relative to repo root.
    fn output_path(&self) -> &'static str;

    /// Generate the artifact content.
    async fn render(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<String>;

    /// Write the artifact to disk, handling merge modes.
    async fn write(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
        repo_root: &Path,
    ) -> crate::error::Result<()> {
        let content = self.render(store, config).await?;
        let output = repo_root.join(self.output_path());

        if output.exists() {
            let existing = std::fs::read_to_string(&output).map_err(|e| {
                crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
            })?;
            let merged = merge_with_preserve(&existing, &content);
            std::fs::write(&output, merged).map_err(|e| {
                crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
            })?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
                })?;
            }
            std::fs::write(&output, content).map_err(|e| {
                crate::error::HomerError::Extract(crate::error::ExtractError::Io(e))
            })?;
        }

        Ok(())
    }
}

/// Merge new content with existing, preserving `<!-- homer:preserve -->` blocks.
pub fn merge_with_preserve(existing: &str, new_content: &str) -> String {
    let preserved = extract_preserved_blocks(existing);

    if preserved.is_empty() {
        return new_content.to_string();
    }

    // Re-insert preserved blocks into the new content
    let mut result = new_content.to_string();
    for block in &preserved {
        // If the new content has the same section header, insert preserved block after it
        if let Some(section) = &block.after_section {
            if let Some(pos) = result.find(section) {
                // Find the end of the section line
                if let Some(newline) = result[pos..].find('\n') {
                    let insert_pos = pos + newline + 1;
                    result.insert_str(insert_pos, &block.content);
                }
            } else {
                // Section not found in new content — append preserved block at end
                result.push('\n');
                result.push_str(&block.content);
            }
        } else {
            // No section context — append at end
            result.push('\n');
            result.push_str(&block.content);
        }
    }

    result
}

struct PreservedBlock {
    content: String,
    after_section: Option<String>,
}

fn extract_preserved_blocks(content: &str) -> Vec<PreservedBlock> {
    let mut blocks = Vec::new();
    let mut current_block: Option<String> = None;
    let mut last_section = None;

    for line in content.lines() {
        if line.trim() == "<!-- homer:preserve -->" {
            current_block = Some(format!("{line}\n"));
        } else if line.trim() == "<!-- /homer:preserve -->" {
            if let Some(mut block) = current_block.take() {
                block.push_str(line);
                block.push('\n');
                blocks.push(PreservedBlock {
                    content: block,
                    after_section: last_section.clone(),
                });
            }
        } else if let Some(ref mut block) = current_block {
            block.push_str(line);
            block.push('\n');
        }

        // Track section headings
        if line.starts_with("## ") {
            last_section = Some(line.to_string());
        }
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_blocks_through_merge() {
        let existing = "# AGENTS.md\n\n## Build\nauto content\n\n## Custom\n<!-- homer:preserve -->\nHuman section\n<!-- /homer:preserve -->\n";
        let new_content = "# AGENTS.md\n\n## Build\nnew auto content\n\n## Custom\nnew auto\n";

        let merged = merge_with_preserve(existing, new_content);
        assert!(
            merged.contains("Human section"),
            "Should preserve human content"
        );
        assert!(
            merged.contains("new auto content"),
            "Should have new auto content"
        );
    }

    #[test]
    fn no_preserve_returns_new() {
        let existing = "# Old\nold content";
        let new_content = "# New\nnew content";
        assert_eq!(merge_with_preserve(existing, new_content), new_content);
    }
}
