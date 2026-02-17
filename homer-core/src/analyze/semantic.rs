// Semantic analyzer: LLM-powered summarization for high-salience entities.
// Gated by composite_salience threshold and doc-comment-aware skip logic.
#![allow(clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::config::HomerConfig;
use crate::llm::cache::{compute_input_hash, get_cached, has_quality_doc_comment, store_cached};
use crate::llm::{AnalysisProvenance, CostTracker, LlmProvider, ProvenanceConfidence};
use crate::store::HomerStore;
use crate::types::{AnalysisKind, NodeKind};

use super::traits::Analyzer;
use super::AnalyzeStats;

const TEMPLATE_VERSION: &str = "entity-summary-v2";

/// Semantic analyzer — calls LLM to summarize high-salience entities.
#[derive(Debug)]
pub struct SemanticAnalyzer {
    provider: Box<dyn LlmProvider>,
}

impl SemanticAnalyzer {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl Analyzer for SemanticAnalyzer {
    fn name(&self) -> &'static str {
        "semantic"
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();
        let mut cost_tracker = CostTracker::default();

        let threshold = config.analysis.llm_salience_threshold;
        let max_batch = config.analysis.max_llm_batch_size as usize;
        let budget = config.llm.cost_budget;

        // Collect high-salience entities
        let candidates = collect_candidates(store, threshold, max_batch).await?;

        if candidates.is_empty() {
            info!("No entities above salience threshold, skipping semantic analysis");
            return Ok(stats);
        }

        info!(
            count = candidates.len(),
            threshold,
            "Running semantic analysis"
        );

        let semaphore = Arc::new(Semaphore::new(config.llm.max_concurrent as usize));

        for candidate in &candidates {
            // Budget check
            if cost_tracker.is_over_budget(budget) {
                warn!(
                    cost = cost_tracker.estimated_cost_usd,
                    budget, "Cost budget exceeded, stopping semantic analysis"
                );
                break;
            }

            match process_entity(
                store,
                &*self.provider,
                candidate,
                &semaphore,
                &mut cost_tracker,
            )
            .await
            {
                Ok(true) => stats.results_stored += 1,
                Ok(false) => {} // skipped (cached or doc-comment)
                Err(e) => {
                    debug!(entity = %candidate.name, error = %e, "Semantic analysis failed");
                    stats
                        .errors
                        .push((candidate.name.clone(), e));
                }
            }
        }

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            cache_hits = cost_tracker.cache_hits,
            doc_skips = cost_tracker.doc_comment_skips,
            cost_usd = cost_tracker.estimated_cost_usd,
            duration = ?stats.duration,
            "Semantic analysis complete"
        );

        Ok(stats)
    }
}

// ── Candidate collection ────────────────────────────────────────────

struct Candidate {
    node_id: crate::types::NodeId,
    name: String,
    salience: f64,
    metadata: serde_json::Map<String, serde_json::Value>,
}

async fn collect_candidates(
    store: &dyn HomerStore,
    threshold: f64,
    max: usize,
) -> crate::error::Result<Vec<Candidate>> {
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;

    let mut candidates: Vec<Candidate> = Vec::new();

    for r in &salience_results {
        let val = r
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        if val < threshold {
            continue;
        }

        // Get the node to check its kind and metadata
        let Some(node) = store.get_node(r.node_id).await? else {
            continue;
        };

        // Only summarize code entities
        match node.kind {
            NodeKind::Function | NodeKind::Type | NodeKind::Module | NodeKind::File => {}
            _ => continue,
        }

        let meta = node.metadata.into_iter().collect();
        candidates.push(Candidate {
            node_id: node.id,
            name: node.name,
            salience: val,
            metadata: meta,
        });
    }

    // Sort by salience descending, take top N
    candidates.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(max);

    Ok(candidates)
}

// ── Entity processing ───────────────────────────────────────────────

/// Returns true if a new result was stored, false if skipped.
async fn process_entity(
    store: &dyn HomerStore,
    provider: &dyn LlmProvider,
    candidate: &Candidate,
    semaphore: &Arc<Semaphore>,
    cost_tracker: &mut CostTracker,
) -> crate::error::Result<bool> {
    let metadata_map = &candidate.metadata;

    // Check doc-comment skip
    if has_quality_doc_comment(metadata_map) {
        cost_tracker.record_doc_skip();
        // Store the doc comment as the summary with high confidence
        let doc = metadata_map
            .get("doc_comment")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        let data = serde_json::json!({
            "summary": doc,
            "provenance": serde_json::to_value(AnalysisProvenance::Algorithmic {
                input_node_ids: vec![candidate.node_id],
            }).unwrap_or_default(),
        });

        store_cached(
            store,
            candidate.node_id,
            AnalysisKind::SemanticSummary,
            data,
            0,
        )
        .await?;

        return Ok(true);
    }

    // Check cache
    let source = metadata_map
        .get("source_snippet")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&candidate.name);

    let input_hash = compute_input_hash(
        provider.model_id(),
        TEMPLATE_VERSION,
        source,
        metadata_map
            .get("doc_comment")
            .and_then(serde_json::Value::as_str),
        &[],
        &[],
    );

    if let Some(_cached) = get_cached(
        store,
        candidate.node_id,
        AnalysisKind::SemanticSummary,
        input_hash,
    )
    .await?
    {
        cost_tracker.record_cache_hit();
        return Ok(false);
    }

    // Build prompt
    let prompt = build_summary_prompt(&candidate.name, source, metadata_map);

    // Acquire semaphore permit for concurrency control
    let _permit = semaphore.acquire().await.map_err(|e| {
        crate::error::HomerError::Analyze(crate::error::AnalyzeError::Computation(e.to_string()))
    })?;

    // Call LLM
    let (response, usage) = provider.call(&prompt, 0.0).await?;
    cost_tracker.record_call(
        &usage,
        provider.cost_per_1k_input(),
        provider.cost_per_1k_output(),
    );

    // Store result
    let provenance = AnalysisProvenance::LlmDerived {
        model_id: provider.model_id().to_string(),
        prompt_template: TEMPLATE_VERSION.to_string(),
        input_hash,
        evidence_nodes: vec![candidate.node_id],
        confidence: ProvenanceConfidence::Medium,
    };

    let data = serde_json::json!({
        "summary": response.trim(),
        "provenance": serde_json::to_value(&provenance).unwrap_or_default(),
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
    });

    store_cached(
        store,
        candidate.node_id,
        AnalysisKind::SemanticSummary,
        data,
        input_hash,
    )
    .await?;

    Ok(true)
}

fn build_summary_prompt(
    name: &str,
    source: &str,
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let kind = metadata
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("entity");

    let file = metadata
        .get("file")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    format!(
        "Summarize this {kind} in one concise sentence (max 100 words). \
         Focus on its purpose and role in the codebase.\n\n\
         Name: {name}\n\
         File: {file}\n\
         Source:\n```\n{source}\n```"
    )
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_includes_name_and_source() {
        let mut meta = serde_json::Map::new();
        meta.insert("kind".to_string(), serde_json::json!("function"));
        meta.insert("file".to_string(), serde_json::json!("src/auth.rs"));

        let prompt = build_summary_prompt("validate_token", "fn validate_token() {}", &meta);
        assert!(prompt.contains("validate_token"));
        assert!(prompt.contains("function"));
        assert!(prompt.contains("src/auth.rs"));
    }

    #[test]
    fn prompt_handles_missing_metadata() {
        let meta = serde_json::Map::new();
        let prompt = build_summary_prompt("foo", "fn foo() {}", &meta);
        assert!(prompt.contains("foo"));
        assert!(prompt.contains("entity")); // default kind
    }
}
