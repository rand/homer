//! Progress reporting for long-running pipeline operations.
//!
//! The CLI uses `IndicatifReporter` for user-visible progress bars.
//! Library callers can use `NoopReporter` or provide their own implementation.

use std::sync::atomic::{AtomicU64, Ordering};

use indicatif::{ProgressBar, ProgressStyle};

/// Trait for reporting progress of pipeline stages.
pub trait ProgressReporter: Send + Sync {
    /// Begin a new task with an optional total count.
    fn start(&self, task: &str, total: Option<u64>);

    /// Advance progress by the given amount.
    fn advance(&self, amount: u64);

    /// Mark the current task as finished.
    fn finish(&self);

    /// Display an informational message.
    fn message(&self, msg: &str);
}

/// No-op reporter for library callers that don't need progress output.
#[derive(Debug, Default)]
pub struct NoopReporter;

impl ProgressReporter for NoopReporter {
    fn start(&self, _task: &str, _total: Option<u64>) {}
    fn advance(&self, _amount: u64) {}
    fn finish(&self) {}
    fn message(&self, _msg: &str) {}
}

/// Reporter backed by `indicatif` progress bars for CLI use.
#[derive(Debug)]
pub struct IndicatifReporter {
    bar: ProgressBar,
    completed: AtomicU64,
}

impl Default for IndicatifReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl IndicatifReporter {
    pub fn new() -> Self {
        Self {
            bar: ProgressBar::hidden(),
            completed: AtomicU64::new(0),
        }
    }
}

impl ProgressReporter for IndicatifReporter {
    fn start(&self, task: &str, total: Option<u64>) {
        self.completed.store(0, Ordering::Relaxed);
        if let Some(total) = total {
            self.bar.set_length(total);
            self.bar
                .set_style(
                    ProgressStyle::with_template(
                        "{spinner:.green} {msg} [{bar:30.cyan/blue}] {pos}/{len} ({eta})",
                    )
                    .unwrap()
                    .progress_chars("=> "),
                );
        } else {
            self.bar.set_length(0);
            self.bar
                .set_style(
                    ProgressStyle::with_template("{spinner:.green} {msg} {pos} items")
                        .unwrap(),
                );
        }
        self.bar.set_message(task.to_string());
        self.bar.reset();
    }

    fn advance(&self, amount: u64) {
        self.completed.fetch_add(amount, Ordering::Relaxed);
        self.bar.inc(amount);
    }

    fn finish(&self) {
        self.bar.finish_and_clear();
    }

    fn message(&self, msg: &str) {
        self.bar.println(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_reporter_is_silent() {
        let reporter = NoopReporter;
        reporter.start("test", Some(100));
        reporter.advance(50);
        reporter.message("hello");
        reporter.finish();
    }

    #[test]
    fn indicatif_reporter_lifecycle() {
        let reporter = IndicatifReporter::new();
        reporter.start("extracting", Some(10));
        reporter.advance(5);
        reporter.advance(5);
        reporter.finish();
    }
}
