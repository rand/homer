use crate::config::HomerConfig;
use crate::store::HomerStore;

use super::AnalyzeStats;

/// Common interface for all analyzers.
#[async_trait::async_trait]
pub trait Analyzer: Send + Sync {
    /// Human-readable name for this analyzer.
    fn name(&self) -> &'static str;

    /// Run analysis, storing results via the store.
    async fn analyze(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats>;
}
