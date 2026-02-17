pub mod behavioral;
pub mod traits;

use std::time::Duration;

/// Statistics returned by an analyzer after a run.
#[derive(Debug, Default)]
pub struct AnalyzeStats {
    pub results_stored: u64,
    pub duration: Duration,
    pub errors: Vec<(String, crate::error::HomerError)>,
}
