use std::time::Duration;

use crate::error::HomerError;

/// Statistics returned by an extractor after a run.
#[derive(Debug, Default)]
pub struct ExtractStats {
    pub nodes_created: u64,
    pub nodes_updated: u64,
    pub edges_created: u64,
    pub duration: Duration,
    pub errors: Vec<(String, HomerError)>,
}

// Extractor trait is defined in P1.03 once HomerStore trait exists.
