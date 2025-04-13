//! Optimism transaction pool metrics

use reth_metrics::{metrics::Histogram, Metrics};
use std::time::Duration;

/// Optimism transaction pool metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "optimism_transaction_pool")]
pub struct SupervisorMetrics {
    /// How long it takes to query the supervisor in the Optimism transaction pool
    pub(crate) supervisor_query_latency: Histogram,
}

impl SupervisorMetrics {
    /// Records the duration of supervisor queries
    #[inline]
    pub fn record_supervisor_query(&self, duration: Duration) {
        self.supervisor_query_latency.record(duration.as_secs_f64());
    }
}