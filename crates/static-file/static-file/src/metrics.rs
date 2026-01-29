//! Static file producer metrics.

use metrics::Histogram;
use reth_metrics::Metrics;
use reth_static_file_types::StaticFileSegment;

/// Metrics for the static file producer.
#[derive(Metrics, Clone)]
#[metrics(scope = "static_file_producer")]
pub(crate) struct StaticFileProducerMetrics {
    /// Histogram of total static file production durations in seconds.
    pub run_duration_seconds: Histogram,
}

/// Returns a histogram for recording segment copy durations.
pub(crate) fn segment_duration_histogram(segment: StaticFileSegment) -> Histogram {
    metrics::histogram!("static_file_producer.segment_duration_seconds", "segment" => segment.to_string())
}
