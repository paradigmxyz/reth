use metrics::{counter, gauge, histogram, Label};
use std::sync::Arc;

/// Represents a type that can report metrics, used mainly with the database. The `report_metrics`
/// method can be used as a prometheus hook.
pub trait DatabaseMetrics {
    /// Reports metrics for the database.
    fn report_metrics(&self) {
        for (name, value, labels) in self.gauge_metrics() {
            gauge!(name, labels).set(value);
        }

        for (name, value, labels) in self.counter_metrics() {
            counter!(name, labels).increment(value);
        }

        for (name, value, labels) in self.histogram_metrics() {
            histogram!(name, labels).record(value);
        }
    }

    /// Returns a list of [Gauge](metrics::Gauge) metrics for the database.
    fn gauge_metrics(&self) -> Vec<(&'static str, f64, Vec<Label>)> {
        vec![]
    }

    /// Returns a list of [Counter](metrics::Counter) metrics for the database.
    fn counter_metrics(&self) -> Vec<(&'static str, u64, Vec<Label>)> {
        vec![]
    }

    /// Returns a list of [Histogram](metrics::Histogram) metrics for the database.
    fn histogram_metrics(&self) -> Vec<(&'static str, f64, Vec<Label>)> {
        vec![]
    }
}

impl<DB: DatabaseMetrics> DatabaseMetrics for Arc<DB> {
    fn report_metrics(&self) {
        <DB as DatabaseMetrics>::report_metrics(self)
    }
}
