pub mod behavioral;
pub mod centrality;
pub mod community;
pub mod convention;
pub mod semantic;
pub mod task_pattern;
pub mod temporal;
pub mod traits;

use std::time::Duration;

/// Statistics returned by an analyzer after a run.
#[derive(Debug, Default)]
pub struct AnalyzeStats {
    pub results_stored: u64,
    pub duration: Duration,
    pub errors: Vec<(String, crate::error::HomerError)>,
}
