//! Static file producer metrics.

use metrics::Histogram;
use reth_metrics::Metrics;

/// Metrics for the static file producer.
#[derive(Metrics, Clone)]
#[metrics(scope = "static_file_producer")]
pub(crate) struct StaticFileProducerMetrics {
    /// Histogram of segment copy durations in seconds.
    pub segment_duration_seconds: Histogram,
    /// Histogram of total static file production durations in seconds.
    pub run_duration_seconds: Histogram,
}
