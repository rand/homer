pub mod cache;
pub mod providers;

use serde::{Deserialize, Serialize};

/// Token usage from an LLM call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Tracks cumulative LLM costs across a pipeline run.
#[allow(clippy::cast_precision_loss)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_requests: u64,
    pub estimated_cost_usd: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub doc_comment_skips: u64,
}

#[allow(clippy::cast_precision_loss)]
impl CostTracker {
    pub fn record_call(
        &mut self,
        usage: &TokenUsage,
        cost_per_1k_input: f64,
        cost_per_1k_output: f64,
    ) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_requests += 1;
        self.estimated_cost_usd += (usage.input_tokens as f64 / 1000.0) * cost_per_1k_input
            + (usage.output_tokens as f64 / 1000.0) * cost_per_1k_output;
    }

    pub fn record_cache_hit(&mut self) {
        self.cache_hits += 1;
    }

    pub fn record_doc_skip(&mut self) {
        self.doc_comment_skips += 1;
    }

    pub fn is_over_budget(&self, budget: f64) -> bool {
        budget > 0.0 && self.estimated_cost_usd >= budget
    }
}

/// Provenance of an analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisProvenance {
    /// Pure algorithmic computation from graph/store data.
    Algorithmic {
        input_node_ids: Vec<crate::types::NodeId>,
    },
    /// Derived via LLM call.
    LlmDerived {
        model_id: String,
        prompt_template: String,
        input_hash: u64,
        evidence_nodes: Vec<crate::types::NodeId>,
        confidence: ProvenanceConfidence,
    },
    /// Combined from multiple sources.
    Composite {
        sources: Vec<AnalysisProvenance>,
    },
}

/// Confidence level for LLM-derived analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceConfidence {
    /// LLM confirmed existing doc comment.
    High,
    /// LLM generated from code alone.
    Medium,
    /// LLM output without specific evidence.
    Low,
}

/// Common interface for LLM providers.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync + std::fmt::Debug {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// The model ID being used.
    fn model_id(&self) -> &str;

    /// Call the LLM with a prompt and return response + token usage.
    async fn call(
        &self,
        prompt: &str,
        temperature: f64,
    ) -> crate::error::Result<(String, TokenUsage)>;

    /// Cost per 1K input tokens (USD).
    fn cost_per_1k_input(&self) -> f64;

    /// Cost per 1K output tokens (USD).
    fn cost_per_1k_output(&self) -> f64;
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_tracker_basics() {
        let mut tracker = CostTracker::default();
        assert!(!tracker.is_over_budget(0.0)); // 0 = unlimited

        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
        };
        tracker.record_call(&usage, 0.003, 0.015);

        assert_eq!(tracker.total_requests, 1);
        assert_eq!(tracker.total_input_tokens, 1000);
        assert_eq!(tracker.total_output_tokens, 500);
        // 1K * 0.003 + 0.5K * 0.015 = 0.003 + 0.0075 = 0.0105
        assert!((tracker.estimated_cost_usd - 0.0105).abs() < 0.0001);
        assert!(!tracker.is_over_budget(1.0));
        assert!(tracker.is_over_budget(0.01));
    }

    #[test]
    fn cost_tracker_cache_skip() {
        let mut tracker = CostTracker::default();
        tracker.record_cache_hit();
        tracker.record_doc_skip();
        assert_eq!(tracker.cache_hits, 1);
        assert_eq!(tracker.doc_comment_skips, 1);
        assert_eq!(tracker.total_requests, 0);
    }

    #[test]
    fn provenance_serde_round_trip() {
        let prov = AnalysisProvenance::LlmDerived {
            model_id: "claude-sonnet-4-20250514".to_string(),
            prompt_template: "entity-summary-v2".to_string(),
            input_hash: 12345,
            evidence_nodes: vec![crate::types::NodeId(1)],
            confidence: ProvenanceConfidence::High,
        };
        let json = serde_json::to_string(&prov).unwrap();
        let back: AnalysisProvenance = serde_json::from_str(&json).unwrap();
        if let AnalysisProvenance::LlmDerived { confidence, .. } = back {
            assert_eq!(confidence, ProvenanceConfidence::High);
        } else {
            panic!("Expected LlmDerived");
        }
    }
}
