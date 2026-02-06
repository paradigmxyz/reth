//! Debug jitter utilities for testing timing-related bugs.
//!
//! When the `debug-jitter` feature is enabled, various components can add
//! random delays to help trigger out-of-order timing bugs that may only
//! manifest in real-world conditions.
//!
//! Control via environment variable:
//! - `RETH_DEBUG_JITTER_MS`: Maximum jitter in milliseconds (0-N random delay)
//!
//! Example: `RETH_DEBUG_JITTER_MS=5` adds 0-5ms random delays.

use std::{sync::OnceLock, thread, time::Duration};

use rand::Rng;
use tracing::trace;

/// Cached jitter configuration from environment.
static JITTER_CONFIG: OnceLock<Option<u64>> = OnceLock::new();

/// Reads the jitter configuration from environment variables.
///
/// Returns `Some(max_ms)` if jitter is enabled, `None` otherwise.
fn get_jitter_config() -> Option<u64> {
    *JITTER_CONFIG.get_or_init(|| {
        std::env::var("RETH_DEBUG_JITTER_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
    })
}

/// Applies a random jitter delay if configured.
///
/// When `RETH_DEBUG_JITTER_MS` is set to a positive value N,
/// this function sleeps for a random duration between 0 and N milliseconds.
///
/// This is useful for testing timing-sensitive code paths that may have
/// race conditions or ordering bugs that only manifest with variable latencies.
///
/// The `context` parameter is used for logging to identify where jitter was applied.
pub fn maybe_apply_jitter(context: &str) {
    if let Some(max_ms) = get_jitter_config() {
        let delay_ms = rand::rng().random_range(0..=max_ms);
        if delay_ms > 0 {
            trace!(
                target: "reth::jitter",
                context,
                delay_ms,
                "Applying debug jitter delay"
            );
            thread::sleep(Duration::from_millis(delay_ms));
        }
    }
}
