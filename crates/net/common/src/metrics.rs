use std::time::Duration;
use metrics::Gauge;
use reth_metrics_derive::Metrics;


/// Network throughput metrics
#[derive(Metrics)]
#[metrics(scope = "network_throughput")]
struct NetworkThroughputMetrics {
    /// Inbound throughput (bytes/s)
    inbound_throughput: Gauge,
    /// Outbound throughput (bytes/s)
    outbound_throughput: Gauge,
}

/// Manages updating the network throughput metrics for a metered stream
pub struct NetworkThroughputManager {
    /// Interval over which the gauges are evaluated
    interval: Duration,
    /// The previously recorded total inbound bandwidth
    prev_inbound: u64,
    /// The previously recorded total outbound bandwidth
    prev_outbound: u64,
    /// Holds the gauges for inbound and outbound throughput
    metrics: NetworkThroughputMetrics,
}

impl NetworkThroughputManager {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            prev_inbound: 0,
            prev_outbound: 0,
            metrics: NetworkThroughputMetrics::default(),
        }
    }

    /// Updates the inbound throughput gauge.
    /// `inbound_bandwidth` is the current total inbound bandwidth through the metered stream
    fn update_inbound_throughput(&mut self, inbound_bandwidth: u64) {
        self.metrics.inbound_throughput.set(
            ((inbound_bandwidth - self.prev_inbound) as f64) / (self.interval.secs as f64)
        );

        self.prev_inbound = inbound_bandwidth;
    }

    /// Updates the outbound throughput gauge.
    /// `outbound_bandwidth` is the current total outbound bandwidth through the metered stream
    fn update_outbound_throughput(&mut self, outbound_bandwidth: u64) {
        self.metrics.outbound_throughput.set(
            ((outbound_bandwidth - self.prev_outbound) as f64) / (self.interval.secs as f64)
        );

        self.prev_outbound = outbound_bandwidth;
    }
}
