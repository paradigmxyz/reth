//! Progress tracking for storage migration.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tracing::info;

/// Progress tracker for migration operations.
pub struct MigrationProgress {
    total: u64,
    completed: AtomicU64,
    start_time: Instant,
    last_log: Instant,
    phase: String,
    log_interval: Duration,
}

impl MigrationProgress {
    /// Create a new progress tracker.
    pub fn new(total: u64) -> Self {
        Self {
            total,
            completed: AtomicU64::new(0),
            start_time: Instant::now(),
            last_log: Instant::now(),
            phase: String::from("Initializing"),
            log_interval: Duration::from_secs(5),
        }
    }

    /// Update completed count.
    pub fn update(&self, delta: u64) {
        self.completed.fetch_add(delta, Ordering::Relaxed);
    }

    /// Set the current phase.
    pub fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
    }

    /// Get current progress percentage.
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        let completed = self.completed.load(Ordering::Relaxed);
        (completed as f64 / self.total as f64 * 100.0).min(100.0)
    }

    /// Get elapsed time.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Estimate remaining time.
    pub fn eta(&self) -> Option<Duration> {
        let completed = self.completed.load(Ordering::Relaxed);
        if completed == 0 {
            return None;
        }

        let elapsed = self.elapsed();
        let rate = completed as f64 / elapsed.as_secs_f64();
        if rate <= 0.0 {
            return None;
        }

        let remaining = self.total.saturating_sub(completed);
        Some(Duration::from_secs_f64(remaining as f64 / rate))
    }

    /// Get processing rate.
    pub fn rate(&self) -> f64 {
        let completed = self.completed.load(Ordering::Relaxed);
        let elapsed = self.elapsed().as_secs_f64();
        if elapsed <= 0.0 {
            0.0
        } else {
            completed as f64 / elapsed
        }
    }

    /// Log progress if enough time has passed.
    pub fn log_progress(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_log) < self.log_interval {
            return;
        }
        self.last_log = now;

        let completed = self.completed.load(Ordering::Relaxed);
        let percent = self.percent();
        let rate = self.rate();
        let eta = self.eta().map(format_duration).unwrap_or_else(|| "calculating".into());

        info!(
            target: "reth::cli::migrate",
            phase = %self.phase,
            completed,
            total = self.total,
            percent = format!("{:.1}%", percent),
            rate = format!("{:.0}/s", rate),
            eta = %eta,
            "Migration progress"
        );
    }

    /// Finish progress tracking.
    pub fn finish(&self) {
        let elapsed = self.elapsed();
        let completed = self.completed.load(Ordering::Relaxed);

        info!(
            target: "reth::cli::migrate",
            completed,
            elapsed_secs = elapsed.as_secs(),
            rate = format!("{:.0}/s", self.rate()),
            "Migration complete"
        );
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}
