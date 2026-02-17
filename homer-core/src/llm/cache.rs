// LLM response cache keyed by (model_id, prompt_template_version, input_hash).
// Uses the store's analysis result table for persistence.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::store::HomerStore;
use crate::types::{AnalysisKind, AnalysisResult, AnalysisResultId, NodeId};

/// Compute a cache key from the inputs that would go into an LLM call.
pub fn compute_input_hash(
    model_id: &str,
    template_version: &str,
    source_code: &str,
    doc_comment: Option<&str>,
    incoming_refs: &[String],
    outgoing_refs: &[String],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    model_id.hash(&mut hasher);
    template_version.hash(&mut hasher);
    source_code.hash(&mut hasher);
    doc_comment.hash(&mut hasher);
    incoming_refs.hash(&mut hasher);
    outgoing_refs.hash(&mut hasher);
    hasher.finish()
}

/// Check if we have a cached result for this node + kind + input hash.
pub async fn get_cached(
    store: &dyn HomerStore,
    node_id: NodeId,
    kind: AnalysisKind,
    input_hash: u64,
) -> crate::error::Result<Option<AnalysisResult>> {
    let existing = store.get_analysis(node_id, kind).await?;
    match existing {
        Some(result) if result.input_hash == input_hash => Ok(Some(result)),
        _ => Ok(None),
    }
}

/// Store a cached LLM result.
pub async fn store_cached(
    store: &dyn HomerStore,
    node_id: NodeId,
    kind: AnalysisKind,
    data: serde_json::Value,
    input_hash: u64,
) -> crate::error::Result<()> {
    store
        .store_analysis(&AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind,
            data,
            input_hash,
            computed_at: chrono::Utc::now(),
        })
        .await
}

/// Determine if an entity has a sufficiently good doc comment to skip LLM.
pub fn has_quality_doc_comment(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(doc) = metadata.get("doc_comment").and_then(serde_json::Value::as_str) else {
        return false;
    };

    // A "quality" doc comment has at least 20 chars and isn't just a placeholder
    let trimmed = doc.trim();
    if trimmed.len() < 20 {
        return false;
    }

    // Reject common placeholder patterns
    let lower = trimmed.to_lowercase();
    let placeholders = ["todo", "fixme", "xxx", "hack", "placeholder"];
    if placeholders.iter().any(|p| lower.starts_with(p)) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_hash_deterministic() {
        let h1 = compute_input_hash("model", "v1", "fn foo() {}", None, &[], &[]);
        let h2 = compute_input_hash("model", "v1", "fn foo() {}", None, &[], &[]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn input_hash_changes_with_model() {
        let h1 = compute_input_hash("model-a", "v1", "code", None, &[], &[]);
        let h2 = compute_input_hash("model-b", "v1", "code", None, &[], &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn input_hash_changes_with_template_version() {
        let h1 = compute_input_hash("model", "v1", "code", None, &[], &[]);
        let h2 = compute_input_hash("model", "v2", "code", None, &[], &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn input_hash_changes_with_code() {
        let h1 = compute_input_hash("model", "v1", "fn a() {}", None, &[], &[]);
        let h2 = compute_input_hash("model", "v1", "fn b() {}", None, &[], &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn quality_doc_comment_detection() {
        let mut meta = serde_json::Map::new();

        // No doc comment
        assert!(!has_quality_doc_comment(&meta));

        // Too short
        meta.insert(
            "doc_comment".to_string(),
            serde_json::Value::String("Short.".to_string()),
        );
        assert!(!has_quality_doc_comment(&meta));

        // Placeholder
        meta.insert(
            "doc_comment".to_string(),
            serde_json::Value::String("TODO: implement this function properly".to_string()),
        );
        assert!(!has_quality_doc_comment(&meta));

        // Quality doc comment
        meta.insert(
            "doc_comment".to_string(),
            serde_json::Value::String(
                "Validates the user token and returns the authenticated user profile."
                    .to_string(),
            ),
        );
        assert!(has_quality_doc_comment(&meta));
    }
}
