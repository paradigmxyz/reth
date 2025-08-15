use metrics;
use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the static file producer.
#[derive(Metrics)]
#[metrics(scope = "static_file_producer")]
pub(crate) struct StaticFileProducerMetrics {
    /// Time taken to process individual segments
    segment_duration_seconds: Histogram,
    /// Time taken for the entire static file producer run
    producer_duration_seconds: Histogram,
}

impl StaticFileProducerMetrics {
    /// Record the duration of processing a single segment
    pub(crate) fn record_segment_duration(&self, duration: std::time::Duration) {
        self.segment_duration_seconds.record(duration.as_secs_f64());
    }

    /// Record the duration of the entire producer run
    pub(crate) fn record_producer_duration(&self, duration: std::time::Duration) {
        self.producer_duration_seconds.record(duration.as_secs_f64());
    }
}
