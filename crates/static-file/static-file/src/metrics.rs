//! Static file producer metrics.

use metrics::Histogram;
use reth_metrics::Metrics;

/// Static file producer metrics.
#[derive(Metrics, Clone)]
#[metrics(scope = "static_file.producer")]
pub(crate) struct StaticFileProducerMetrics {
    /// Histogram for the time taken to produce static files for a single segment (in seconds).
    pub(crate) segment_production_duration: Histogram,
    /// Histogram for the total time taken to produce all static files (in seconds).
    pub(crate) total_production_duration: Histogram,
}
