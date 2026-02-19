use std::time::Duration;

use crate::config::HomerConfig;
use crate::error::HomerError;
use crate::store::HomerStore;

/// Statistics returned by an extractor after a run.
#[derive(Debug, Default)]
pub struct ExtractStats {
    pub nodes_created: u64,
    pub nodes_updated: u64,
    pub edges_created: u64,
    pub duration: Duration,
    pub errors: Vec<(String, HomerError)>,
}

/// Common interface for all extractors.
///
/// Uses `?Send` because gix types (Repository, Commit) contain `RefCell`
/// and cannot be held across await points in a Send future.
#[async_trait::async_trait(?Send)]
pub trait Extractor {
    /// Human-readable name for this extractor (e.g. "git", "structure").
    fn name(&self) -> &'static str;

    /// Check if this extractor has new data to process since the last run.
    /// Default: always returns `true` (conservative).
    async fn has_work(&self, _store: &dyn HomerStore) -> crate::error::Result<bool> {
        Ok(true)
    }

    /// Run extraction, populating the store with nodes and edges.
    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats>;
}
