//! Histogram bucket configuration registry for metrics.
//!
//! This module provides a way for metrics to register custom histogram buckets
//! without modifying the recorder installation code.

use metrics_exporter_prometheus::Matcher;
use std::sync::LazyLock;

/// A histogram bucket configuration.
#[derive(Debug, Clone)]
pub struct BucketConfig {
    /// The matcher for the metric name.
    pub matcher: Matcher,
    /// The bucket boundaries.
    pub buckets: Vec<f64>,
}

impl BucketConfig {
    /// Create a new bucket configuration with a full metric name match.
    pub fn new(metric_name: impl Into<String>, buckets: Vec<f64>) -> Self {
        Self { matcher: Matcher::Full(metric_name.into()), buckets }
    }

    /// Create a new bucket configuration with a prefix match.
    pub fn new_prefix(prefix: impl Into<String>, buckets: Vec<f64>) -> Self {
        Self { matcher: Matcher::Prefix(prefix.into()), buckets }
    }

    /// Create a new bucket configuration with a suffix match.
    pub fn new_suffix(suffix: impl Into<String>, buckets: Vec<f64>) -> Self {
        Self { matcher: Matcher::Suffix(suffix.into()), buckets }
    }
}

/// Global registry for histogram bucket configurations.
static BUCKET_REGISTRY: LazyLock<parking_lot::RwLock<Vec<BucketConfig>>> =
    LazyLock::new(|| parking_lot::RwLock::new(Vec::new()));

/// Register custom histogram buckets for a metric.
///
/// This should be called during metric initialization, before the prometheus recorder
/// is installed.
///
/// # Example
///
/// ```
/// use reth_node_metrics::buckets::{register_buckets, BucketConfig};
///
/// // Register buckets for a specific metric
/// register_buckets(BucketConfig::new(
///     "reth_ef_execution_inclusion_list_transactions_validation_time_seconds",
///     vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0],
/// ));
/// ```
pub fn register_buckets(config: BucketConfig) {
    BUCKET_REGISTRY.write().push(config);
}

/// Register multiple bucket configurations at once.
pub fn register_buckets_batch(configs: impl IntoIterator<Item = BucketConfig>) {
    let mut registry = BUCKET_REGISTRY.write();
    registry.extend(configs);
}

/// Register default histogram buckets for all Reth metrics.
///
/// This is called automatically when the Prometheus recorder is installed.
/// It registers buckets for all metrics that need custom bucket configurations.
pub(crate) fn register_default_histogram_buckets() {
    // Register FOCIL/EF execution metrics buckets
    register_ef_execution_buckets();
}

/// Register histogram buckets for EF execution metrics.
fn register_ef_execution_buckets() {
    use presets::DURATION_MICROS_TO_SECS;

    // Histogram: validation time in seconds (microsecond to multi-second range)
    register_buckets(BucketConfig::new(
        "reth_ef_execution_inclusion_list_transactions_validation_time_seconds",
        DURATION_MICROS_TO_SECS.to_vec(),
    ));
}

/// Get all registered bucket configurations.
///
/// This is used internally by the recorder installation code.
pub(crate) fn get_registered_buckets() -> Vec<BucketConfig> {
    BUCKET_REGISTRY.read().clone()
}

/// Clear all registered buckets. Primarily useful for testing.
#[cfg(test)]
pub fn clear_registered_buckets() {
    BUCKET_REGISTRY.write().clear();
}

/// Common bucket configurations for typical use cases.
pub mod presets {
    /// Buckets for sub-millisecond to multi-second durations.
    ///
    /// Useful for validation, execution, or processing times.
    pub const DURATION_MICROS_TO_SECS: &[f64] = &[
        0.0001, // 100 microseconds
        0.0005, // 500 microseconds
        0.001,  // 1 millisecond
        0.005,  // 5 milliseconds
        0.01,   // 10 milliseconds
        0.05,   // 50 milliseconds
        0.1,    // 100 milliseconds
        0.5,    // 500 milliseconds
        1.0,    // 1 second
        2.0,    // 2 seconds
        5.0,    // 5 seconds
    ];

    /// Buckets for millisecond to minute durations.
    pub const DURATION_MILLIS_TO_MINS: &[f64] = &[
        0.001, // 1 millisecond
        0.01,  // 10 milliseconds
        0.1,   // 100 milliseconds
        0.5,   // 500 milliseconds
        1.0,   // 1 second
        5.0,   // 5 seconds
        10.0,  // 10 seconds
        30.0,  // 30 seconds
        60.0,  // 1 minute
    ];

    /// Buckets for seconds to hours.
    pub const DURATION_SECS_TO_HOURS: &[f64] = &[
        1.0,    // 1 second
        10.0,   // 10 seconds
        60.0,   // 1 minute
        300.0,  // 5 minutes
        600.0,  // 10 minutes
        1800.0, // 30 minutes
        3600.0, // 1 hour
    ];

    /// Buckets for byte sizes from bytes to gigabytes.
    pub const BYTES_TO_GB: &[f64] = &[
        1024.0,                   // 1 KB
        1024.0 * 1024.0,          // 1 MB
        10.0 * 1024.0 * 1024.0,   // 10 MB
        100.0 * 1024.0 * 1024.0,  // 100 MB
        1024.0 * 1024.0 * 1024.0, // 1 GB
    ];

    /// Buckets for small counts (1-1000).
    pub const COUNT_SMALL: &[f64] = &[1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];

    /// Buckets for medium counts (1-100k).
    pub const COUNT_MEDIUM: &[f64] =
        &[1.0, 10.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 50000.0, 100000.0];

    /// Buckets for large counts (1-10M).
    pub const COUNT_LARGE: &[f64] =
        &[1.0, 100.0, 1000.0, 10000.0, 100000.0, 500000.0, 1000000.0, 5000000.0, 10000000.0];
}

#[cfg(test)]
mod tests {
    use super::*;

    // Use a test lock to serialize tests that modify global state
    static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn test_bucket_registration() {
        let _lock = TEST_LOCK.lock();
        clear_registered_buckets();

        let config = BucketConfig::new("test_metric", vec![1.0, 2.0, 3.0]);
        register_buckets(config);

        let registered = get_registered_buckets();

        // Find our test bucket
        let test_bucket = registered.iter().find(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Full(name) if name == "test_metric")
        });
        assert!(test_bucket.is_some(), "Test bucket not found in registry");
        assert_eq!(test_bucket.unwrap().buckets, vec![1.0, 2.0, 3.0]);

        clear_registered_buckets();
    }

    #[test]
    fn test_batch_registration() {
        let _lock = TEST_LOCK.lock();
        clear_registered_buckets();

        let configs = vec![
            BucketConfig::new("metric1", vec![1.0, 2.0]),
            BucketConfig::new("metric2", vec![3.0, 4.0]),
        ];
        register_buckets_batch(configs);

        let registered = get_registered_buckets();

        // Check that our metrics are present
        let has_metric1 = registered.iter().any(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Full(name) if name == "metric1")
        });
        let has_metric2 = registered.iter().any(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Full(name) if name == "metric2")
        });

        assert!(has_metric1, "metric1 not found");
        assert!(has_metric2, "metric2 not found");

        clear_registered_buckets();
    }

    #[test]
    fn test_matcher_types() {
        let _lock = TEST_LOCK.lock();
        clear_registered_buckets();

        register_buckets(BucketConfig::new("full_match", vec![1.0]));
        register_buckets(BucketConfig::new_prefix("prefix_", vec![2.0]));
        register_buckets(BucketConfig::new_suffix("_suffix", vec![3.0]));

        let registered = get_registered_buckets();

        // Verify all three matcher types are present
        let has_full = registered.iter().any(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Full(name) if name == "full_match")
        });
        let has_prefix = registered.iter().any(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Prefix(name) if name == "prefix_")
        });
        let has_suffix = registered.iter().any(|b| {
            matches!(&b.matcher, metrics_exporter_prometheus::Matcher::Suffix(name) if name == "_suffix")
        });

        assert!(has_full, "Full matcher not found");
        assert!(has_prefix, "Prefix matcher not found");
        assert!(has_suffix, "Suffix matcher not found");

        clear_registered_buckets();
    }
}
