use std::{collections::HashMap, sync::Arc};

use metrics::{counter, gauge, histogram, Label};

/// Represents a type that can report metrics, used mainly with the database. The `report_metrics`
/// method can be used as a prometheus hook.
pub trait DatabaseMetrics {
    /// Reports metrics for the database.
    fn report_metrics(&self) {
        for (name, value, labels) in self.gauge_metrics() {
            gauge!(name, value, labels);
        }

        for (name, value, labels) in self.counter_metrics() {
            counter!(name, value, labels);
        }

        for (name, value, labels) in self.histogram_metrics() {
            histogram!(name, value, labels);
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

/// Extends [Database] to include a [Metadata] type, which can be used by methods which need to
/// dynamically retrieve information about the database.
pub trait DatabaseMetadata: Database {
    /// The type used to store metadata about the database.
    type Metadata;

    /// Returns a [HashMap] of [&'static str]-addressable metadata about the database.
    fn metadata(&self) -> HashMap<&'static str, Self::Metadata>;
}

impl<DB: DatabaseMetadata> DatabaseMetadata for Arc<DB> {
    type Metadata = DB::Metadata;

    fn metadata(&self) -> HashMap<&'static str, Self::Metadata> {
        <DB as DatabaseMetadata>::metadata(self)
    }
}
