use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::AnalysisKind;

use super::AnalyzeStats;

/// Common interface for all analyzers.
#[async_trait::async_trait]
pub trait Analyzer: Send + Sync {
    /// Human-readable name for this analyzer.
    fn name(&self) -> &'static str;

    /// What `AnalysisKind`s this analyzer writes to the store.
    fn produces(&self) -> &'static [AnalysisKind] {
        &[]
    }

    /// What `AnalysisKind`s must already exist in the store before this runs.
    fn requires(&self) -> &'static [AnalysisKind] {
        &[]
    }

    /// Check if this analyzer needs to re-run (any inputs changed since last run).
    /// Default: always returns `true` (conservative).
    async fn needs_rerun(&self, _store: &dyn HomerStore) -> crate::error::Result<bool> {
        Ok(true)
    }

    /// Run analysis, storing results via the store.
    async fn analyze(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats>;
}
