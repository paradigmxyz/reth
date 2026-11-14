//! Configuration of a [`PrometheusRecorder`].

use super::{
    distribution::{
        DistributionBuilder, DEFAULT_SUMMARY_BUCKET_COUNT, DEFAULT_SUMMARY_BUCKET_DURATION,
    },
    formatting::sanitize_metric_name,
    PrometheusRecorder,
};
use std::{num::NonZeroU32, time::Duration};

/// Quantiles rendered for summaries unless overridden with
/// [`PrometheusBuilder::set_quantiles`].
const DEFAULT_QUANTILES: [f64; 7] = [0.0, 0.5, 0.9, 0.95, 0.99, 0.999, 1.0];

/// Builder for a [`PrometheusRecorder`].
///
/// By default every histogram is rendered as a summary with the quantiles `0`, `0.5`, `0.9`,
/// `0.95`, `0.99`, `0.999` and `1` over a rolling window of three 20 second buckets.
#[derive(Debug)]
pub struct PrometheusBuilder {
    quantiles: Vec<f64>,
    bucket_duration: Duration,
    bucket_count: NonZeroU32,
    buckets: Option<Vec<f64>>,
    bucket_overrides: Vec<(Matcher, Vec<f64>)>,
    global_labels: Vec<(String, String)>,
    prefix: Option<&'static str>,
}

impl PrometheusBuilder {
    /// Creates a builder with the default configuration.
    pub fn new() -> Self {
        Self {
            quantiles: DEFAULT_QUANTILES.to_vec(),
            bucket_duration: DEFAULT_SUMMARY_BUCKET_DURATION,
            bucket_count: DEFAULT_SUMMARY_BUCKET_COUNT,
            buckets: None,
            bucket_overrides: Vec::new(),
            global_labels: Vec::new(),
            prefix: None,
        }
    }

    /// Sets the quantiles rendered for summaries. Values are clamped to `[0, 1]`.
    ///
    /// Returns [`BuildError::EmptyBucketsOrQuantiles`] if `quantiles` is empty.
    pub fn set_quantiles(mut self, quantiles: &[f64]) -> Result<Self, BuildError> {
        if quantiles.is_empty() {
            return Err(BuildError::EmptyBucketsOrQuantiles);
        }
        self.quantiles = quantiles.iter().map(|q| q.clamp(0.0, 1.0)).collect();
        Ok(self)
    }

    /// Sets the duration of a single rolling summary bucket.
    ///
    /// Summaries cover `bucket_count * bucket_duration`. Returns
    /// [`BuildError::ZeroBucketDuration`] if `value` is zero.
    pub fn set_bucket_duration(mut self, value: Duration) -> Result<Self, BuildError> {
        if value.is_zero() {
            return Err(BuildError::ZeroBucketDuration);
        }
        self.bucket_duration = value;
        Ok(self)
    }

    /// Sets the number of rolling summary buckets.
    pub const fn set_bucket_count(mut self, count: NonZeroU32) -> Self {
        self.bucket_count = count;
        self
    }

    /// Renders every histogram as a Prometheus histogram with these bucket upper bounds instead of
    /// a summary.
    ///
    /// Returns [`BuildError::EmptyBucketsOrQuantiles`] if `values` is empty.
    pub fn set_buckets(mut self, values: &[f64]) -> Result<Self, BuildError> {
        if values.is_empty() {
            return Err(BuildError::EmptyBucketsOrQuantiles);
        }
        self.buckets = Some(values.to_vec());
        Ok(self)
    }

    /// Renders histograms whose sanitized name matches `matcher` as Prometheus histograms with
    /// these bucket upper bounds, taking precedence over [`Self::set_buckets`].
    ///
    /// When several matchers match, `Full` wins over `Prefix`, which wins over `Suffix`. Setting
    /// buckets for the same matcher again replaces them. Returns
    /// [`BuildError::EmptyBucketsOrQuantiles`] if `values` is empty.
    pub fn set_buckets_for_metric(
        mut self,
        matcher: Matcher,
        values: &[f64],
    ) -> Result<Self, BuildError> {
        if values.is_empty() {
            return Err(BuildError::EmptyBucketsOrQuantiles);
        }
        let matcher = matcher.sanitized();
        self.bucket_overrides.retain(|(existing, _)| *existing != matcher);
        self.bucket_overrides.push((matcher, values.to_vec()));
        Ok(self)
    }

    /// Adds a label to every series. Labels of a metric key override global labels with the same
    /// key, and adding the same key again replaces its value.
    pub fn add_global_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let (key, value) = (key.into(), value.into());
        if let Some(existing) = self.global_labels.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = value;
        } else {
            self.global_labels.push((key, value));
        }
        self
    }

    /// Prepends `<prefix>.` to every metric name and description before sanitization, so
    /// `foo.bar` with prefix `reth` renders as `reth_foo_bar`. Bucket matchers see the prefixed
    /// name.
    pub const fn with_prefix(mut self, prefix: &'static str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    /// Builds the recorder without installing it.
    pub fn build_recorder(self) -> PrometheusRecorder {
        let distribution_builder = DistributionBuilder::new(
            self.quantiles,
            self.buckets,
            self.bucket_duration,
            self.bucket_count,
            self.bucket_overrides,
        );
        PrometheusRecorder::new(distribution_builder, self.global_labels, self.prefix)
    }
}

impl Default for PrometheusBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Matches a sanitized metric name, used for per-metric bucket overrides.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Matcher {
    /// Matches the entire metric name.
    Full(String),
    /// Matches the beginning of the metric name.
    Prefix(String),
    /// Matches the end of the metric name.
    Suffix(String),
}

impl Matcher {
    /// Returns whether `key` matches.
    pub fn matches(&self, key: &str) -> bool {
        match self {
            Self::Prefix(prefix) => key.starts_with(prefix),
            Self::Suffix(suffix) => key.ends_with(suffix),
            Self::Full(full) => key == full,
        }
    }

    fn sanitized(self) -> Self {
        match self {
            Self::Prefix(prefix) => Self::Prefix(sanitize_metric_name(&prefix)),
            Self::Suffix(suffix) => Self::Suffix(sanitize_metric_name(&suffix)),
            Self::Full(full) => Self::Full(sanitize_metric_name(&full)),
        }
    }
}

/// Errors returned by [`PrometheusBuilder`] setters.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Bucket bounds or quantiles were empty.
    #[error("bucket bounds/quantiles cannot be empty")]
    EmptyBucketsOrQuantiles,
    /// The rolling summary bucket duration was zero.
    #[error("bucket durations cannot be set to zero")]
    ZeroBucketDuration,
}
