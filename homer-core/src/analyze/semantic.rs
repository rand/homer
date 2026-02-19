// Semantic analyzer: LLM-powered summarization for high-salience entities.
// Gated by composite_salience threshold and doc-comment-aware skip logic.
#![allow(clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tracing::{debug, info, instrument, warn};

use crate::config::HomerConfig;
use crate::llm::cache::{compute_input_hash, get_cached, has_quality_doc_comment, store_cached};
use crate::llm::{AnalysisProvenance, CostTracker, LlmProvider, ProvenanceConfidence};
use crate::store::HomerStore;
use crate::types::{AnalysisKind, NodeKind};

use super::AnalyzeStats;
use super::traits::Analyzer;

const TEMPLATE_VERSION: &str = "entity-summary-v3";
const RATIONALE_TEMPLATE_VERSION: &str = "design-rationale-v1";

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

    fn produces(&self) -> &'static [AnalysisKind] {
        &[
            AnalysisKind::SemanticSummary,
            AnalysisKind::InvariantDescription,
            AnalysisKind::DesignRationale,
        ]
    }

    fn requires(&self) -> &'static [AnalysisKind] {
        &[
            AnalysisKind::CompositeSalience,
            AnalysisKind::PageRank,
            AnalysisKind::BetweennessCentrality,
        ]
    }

    #[instrument(skip_all, name = "semantic_analyze")]
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
            threshold, "Running semantic analysis"
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
                    stats.errors.push((candidate.name.clone(), e));
                }
            }
        }

        // Design rationale extraction for merged PRs
        if !cost_tracker.is_over_budget(budget) {
            let rationale_count = extract_design_rationales(
                store,
                &*self.provider,
                &semaphore,
                &mut cost_tracker,
                budget,
            )
            .await;
            stats.results_stored += rationale_count;
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
    classification: String,
    pagerank: f64,
    betweenness: f64,
    callers: Vec<String>,
    callees: Vec<String>,
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

    // Pre-load centrality data for enrichment
    let pagerank_results = store.get_analyses_by_kind(AnalysisKind::PageRank).await?;
    let betweenness_results = store
        .get_analyses_by_kind(AnalysisKind::BetweennessCentrality)
        .await?;

    let pagerank_map: std::collections::HashMap<_, f64> = pagerank_results
        .iter()
        .filter_map(|r| {
            let v = r.data.get("score").and_then(serde_json::Value::as_f64)?;
            Some((r.node_id, v))
        })
        .collect();
    let betweenness_map: std::collections::HashMap<_, f64> = betweenness_results
        .iter()
        .filter_map(|r| {
            let v = r.data.get("score").and_then(serde_json::Value::as_f64)?;
            Some((r.node_id, v))
        })
        .collect();

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

        let Some(node) = store.get_node(r.node_id).await? else {
            continue;
        };

        match node.kind {
            NodeKind::Function | NodeKind::Type | NodeKind::Module | NodeKind::File => {}
            _ => continue,
        }

        let classification = r
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown")
            .to_string();

        // Get callers/callees from call graph edges
        let (incoming, outgoing) = collect_call_neighbors(store, &node).await;

        let meta = node.metadata.into_iter().collect();
        candidates.push(Candidate {
            node_id: node.id,
            name: node.name,
            salience: val,
            classification,
            pagerank: pagerank_map.get(&node.id).copied().unwrap_or(0.0),
            betweenness: betweenness_map.get(&node.id).copied().unwrap_or(0.0),
            callers: incoming,
            callees: outgoing,
            metadata: meta,
        });
    }

    candidates.sort_by(|a, b| {
        b.salience
            .partial_cmp(&a.salience)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(max);

    Ok(candidates)
}

async fn collect_call_neighbors(
    store: &dyn HomerStore,
    node: &crate::types::Node,
) -> (Vec<String>, Vec<String>) {
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    if let Ok(edges) = store.get_edges_involving(node.id).await {
        for edge in &edges {
            if edge.kind != crate::types::HyperedgeKind::Calls {
                continue;
            }
            for member in &edge.members {
                if member.node_id == node.id {
                    continue;
                }
                if let Ok(Some(m_node)) = store.get_node(member.node_id).await {
                    if member.role == "caller" {
                        incoming.push(m_node.name);
                    } else if member.role == "callee" {
                        outgoing.push(m_node.name);
                    }
                }
            }
        }
    }
    (incoming, outgoing)
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

    // Build prompt using full spec template
    let prompt = build_summary_prompt(candidate, source);

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

    // Parse structured JSON response; fall back to plain summary
    let parsed: serde_json::Value = serde_json::from_str(response.trim())
        .unwrap_or_else(|_| serde_json::json!({"summary": response.trim()}));

    let provenance = AnalysisProvenance::LlmDerived {
        model_id: provider.model_id().to_string(),
        prompt_template: TEMPLATE_VERSION.to_string(),
        input_hash,
        evidence_nodes: vec![candidate.node_id],
        confidence: ProvenanceConfidence::Medium,
    };

    // Store SemanticSummary
    let data = serde_json::json!({
        "summary": parsed.get("summary").and_then(serde_json::Value::as_str).unwrap_or(response.trim()),
        "usage_pattern": parsed.get("usage_pattern"),
        "caution": parsed.get("caution"),
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

    store_invariants(store, &parsed, candidate.node_id, &provenance, input_hash).await?;

    Ok(true)
}

async fn store_invariants(
    store: &dyn HomerStore,
    parsed: &serde_json::Value,
    node_id: crate::types::NodeId,
    provenance: &AnalysisProvenance,
    input_hash: u64,
) -> crate::error::Result<()> {
    if let Some(invariants) = parsed.get("invariants") {
        if invariants.is_array() && !invariants.as_array().unwrap_or(&Vec::new()).is_empty() {
            let inv_data = serde_json::json!({
                "invariants": invariants,
                "provenance": serde_json::to_value(provenance).unwrap_or_default(),
            });
            store_cached(
                store,
                node_id,
                AnalysisKind::InvariantDescription,
                inv_data,
                input_hash,
            )
            .await?;
        }
    }
    Ok(())
}

#[allow(clippy::similar_names)]
fn build_summary_prompt(candidate: &Candidate, source: &str) -> String {
    let kind = candidate
        .metadata
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("entity");

    let file = candidate
        .metadata
        .get("file")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let doc_comment = candidate
        .metadata
        .get("doc_comment")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("(none)");

    let language = candidate
        .metadata
        .get("language")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let callers_display = if candidate.callers.is_empty() {
        "(none)".to_string()
    } else {
        candidate
            .callers
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let callees_display = if candidate.callees.is_empty() {
        "(none)".to_string()
    } else {
        candidate
            .callees
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "You are analyzing a code entity for a repository mining tool. \
         Produce a concise summary optimized for AI coding agents.\n\n\
         Entity: {name}\n\
         Kind: {kind}\n\
         File: {file}\n\
         Centrality: PageRank={pagerank:.4}, Betweenness={betweenness:.4}\n\
         Classification: {classification}\n\
         Called by: {callers} ({caller_count} total)\n\
         Calls: {callees} ({callee_count} total)\n\
         Doc comment: {doc_comment}\n\n\
         Source code:\n```{language}\n{source}\n```\n\n\
         Produce a JSON response with:\n\
         1. \"summary\": 1-2 sentence description of what this entity does\n\
         2. \"invariants\": list of apparent invariants (things that must remain true)\n\
         3. \"usage_pattern\": how callers typically use this entity\n\
         4. \"caution\": any warnings for an agent that might modify this code",
        name = candidate.name,
        pagerank = candidate.pagerank,
        betweenness = candidate.betweenness,
        classification = candidate.classification,
        callers = callers_display,
        caller_count = candidate.callers.len(),
        callees = callees_display,
        callee_count = candidate.callees.len(),
    )
}

#[allow(clippy::too_many_lines)]
async fn extract_design_rationales(
    store: &dyn HomerStore,
    provider: &dyn LlmProvider,
    semaphore: &Semaphore,
    cost_tracker: &mut CostTracker,
    budget: f64,
) -> u64 {
    let Ok(prs) = store
        .find_nodes(&crate::types::NodeFilter {
            kind: Some(NodeKind::PullRequest),
            ..Default::default()
        })
        .await
    else {
        return 0;
    };

    let mut count = 0u64;

    for pr in &prs {
        if cost_tracker.is_over_budget(budget) {
            break;
        }

        // Skip if already analyzed or not a merged PR with body
        if let Ok(Some(_)) = store
            .get_analysis(pr.id, AnalysisKind::DesignRationale)
            .await
        {
            continue;
        }
        let Some(body) = pr.metadata.get("body").and_then(|v| v.as_str()) else {
            continue;
        };
        if !pr.metadata.contains_key("merged_at") {
            continue;
        }

        let title = pr
            .metadata
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&pr.name);
        let number = pr
            .metadata
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        let prompt = build_rationale_prompt(number, title, body, &[], &[]);
        let input_hash = compute_input_hash(
            provider.model_id(),
            RATIONALE_TEMPLATE_VERSION,
            &prompt,
            None,
            &[],
            &[],
        );

        // Check cache
        if let Ok(Some(_)) =
            get_cached(store, pr.id, AnalysisKind::DesignRationale, input_hash).await
        {
            cost_tracker.record_cache_hit();
            continue;
        }

        let Ok(_permit) = semaphore.acquire().await else {
            break;
        };

        match provider.call(&prompt, 0.0).await {
            Ok((response, usage)) => {
                cost_tracker.record_call(
                    &usage,
                    provider.cost_per_1k_input(),
                    provider.cost_per_1k_output(),
                );

                let parsed: serde_json::Value = serde_json::from_str(response.trim())
                    .unwrap_or_else(|_| serde_json::json!({"rationale": response.trim()}));

                let provenance = AnalysisProvenance::LlmDerived {
                    model_id: provider.model_id().to_string(),
                    prompt_template: RATIONALE_TEMPLATE_VERSION.to_string(),
                    input_hash,
                    evidence_nodes: vec![pr.id],
                    confidence: ProvenanceConfidence::Medium,
                };

                let data = serde_json::json!({
                    "pr_number": number,
                    "title": title,
                    "motivation": parsed.get("motivation"),
                    "approach": parsed.get("approach"),
                    "alternatives_considered": parsed.get("alternatives_considered"),
                    "tradeoffs": parsed.get("tradeoffs"),
                    "provenance": serde_json::to_value(&provenance).unwrap_or_default(),
                });

                if store_cached(
                    store,
                    pr.id,
                    AnalysisKind::DesignRationale,
                    data,
                    input_hash,
                )
                .await
                .is_ok()
                {
                    count += 1;
                }
            }
            Err(e) => {
                debug!(pr = %pr.name, error = %e, "Design rationale extraction failed");
            }
        }
    }

    count
}

fn build_rationale_prompt(
    pr_number: u64,
    title: &str,
    body: &str,
    files: &[String],
    review_comments: &[String],
) -> String {
    let files_display = files.join(", ");
    let comments_display = if review_comments.is_empty() {
        "(none)".to_string()
    } else {
        review_comments.join("\n---\n")
    };

    format!(
        "This PR made significant changes to important code. \
         Extract the design rationale.\n\n\
         PR #{pr_number}: {title}\n\
         Description: {body}\n\
         Files changed: {files_display}\n\
         Review comments: {comments_display}\n\n\
         Produce a JSON response with:\n\
         1. \"motivation\": why this change was made\n\
         2. \"approach\": what approach was chosen\n\
         3. \"alternatives_considered\": any alternatives mentioned\n\
         4. \"tradeoffs\": explicit or implicit tradeoffs"
    )
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_candidate(name: &str, meta: serde_json::Map<String, serde_json::Value>) -> Candidate {
        Candidate {
            node_id: crate::types::NodeId(1),
            name: name.to_string(),
            salience: 0.8,
            classification: "HotCritical".to_string(),
            pagerank: 0.05,
            betweenness: 0.12,
            callers: vec!["caller_a".to_string()],
            callees: vec!["callee_b".to_string()],
            metadata: meta,
        }
    }

    #[test]
    fn build_prompt_includes_name_and_source() {
        let mut meta = serde_json::Map::new();
        meta.insert("kind".to_string(), serde_json::json!("function"));
        meta.insert("file".to_string(), serde_json::json!("src/auth.rs"));

        let candidate = test_candidate("validate_token", meta);
        let prompt = build_summary_prompt(&candidate, "fn validate_token() {}");
        assert!(prompt.contains("validate_token"));
        assert!(prompt.contains("function"));
        assert!(prompt.contains("src/auth.rs"));
        assert!(prompt.contains("PageRank=0.0500"));
        assert!(prompt.contains("Betweenness=0.1200"));
        assert!(prompt.contains("caller_a"));
        assert!(prompt.contains("callee_b"));
    }

    #[test]
    fn prompt_handles_missing_metadata() {
        let meta = serde_json::Map::new();
        let candidate = test_candidate("foo", meta);
        let prompt = build_summary_prompt(&candidate, "fn foo() {}");
        assert!(prompt.contains("foo"));
        assert!(prompt.contains("entity")); // default kind
    }

    #[test]
    fn rationale_prompt_includes_pr_info() {
        let prompt = build_rationale_prompt(
            42,
            "Refactor auth flow",
            "Switched from JWT to session tokens",
            &["src/auth.rs".to_string(), "src/session.rs".to_string()],
            &["Consider rate limiting".to_string()],
        );
        assert!(prompt.contains("PR #42"));
        assert!(prompt.contains("Refactor auth flow"));
        assert!(prompt.contains("session tokens"));
        assert!(prompt.contains("src/auth.rs"));
        assert!(prompt.contains("rate limiting"));
    }
}
