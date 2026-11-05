//! Aggregation of histogram samples into Prometheus summaries and histograms.
//!
//! Ported from `metrics-exporter-prometheus` 0.18.3 (`src/distribution.rs`) and `metrics-util`
//! 0.20.4 (`src/storage/{histogram,summary}.rs`), MIT licensed, Copyright (c) 2021 Metrics
//! Contributors. Quantiles, windows and counts match upstream exactly.

use super::builder::Matcher;
use quanta::Instant;
use sketches_ddsketch::{Config, DDSketch};
use std::{num::NonZeroU32, sync::Arc, time::Duration};

/// Default number of rolling summary buckets.
pub(super) const DEFAULT_SUMMARY_BUCKET_COUNT: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// Default duration of a single rolling summary bucket.
pub(super) const DEFAULT_SUMMARY_BUCKET_DURATION: Duration = Duration::from_secs(20);

/// Aggregated samples of one histogram series.
#[derive(Debug)]
pub(super) enum Distribution {
    /// Cumulative buckets with fixed upper bounds.
    Histogram(Histogram),
    /// Quantiles over a rolling window, plus the cumulative sum of all samples.
    Summary(RollingSummary, Arc<[f64]>, f64),
}

impl Distribution {
    /// Records drained `(value, timestamp)` samples.
    pub(super) fn record_samples(&mut self, samples: &[(f64, Instant)]) {
        match self {
            Self::Histogram(hist) => hist.record_many(samples.iter().map(|(sample, _)| *sample)),
            Self::Summary(summary, _, sum) => {
                for &(sample, ts) in samples {
                    summary.add(sample, ts);
                    *sum += sample;
                }
            }
        }
    }
}

/// Picks the distribution kind for a metric name from the builder configuration.
#[derive(Debug)]
pub(super) struct DistributionBuilder {
    quantiles: Arc<[f64]>,
    buckets: Option<Vec<f64>>,
    bucket_duration: Duration,
    bucket_count: NonZeroU32,
    /// Sorted by [`Matcher`] so the first match is deterministic.
    bucket_overrides: Vec<(Matcher, Vec<f64>)>,
}

impl DistributionBuilder {
    pub(super) fn new(
        quantiles: Vec<f64>,
        buckets: Option<Vec<f64>>,
        bucket_duration: Duration,
        bucket_count: NonZeroU32,
        mut bucket_overrides: Vec<(Matcher, Vec<f64>)>,
    ) -> Self {
        bucket_overrides.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            quantiles: quantiles.into(),
            buckets,
            bucket_duration,
            bucket_count,
            bucket_overrides,
        }
    }

    /// Returns an empty distribution for the sanitized metric `name`.
    pub(super) fn get_distribution(&self, name: &str) -> Distribution {
        if let Some((_, buckets)) = self.bucket_overrides.iter().find(|(m, _)| m.matches(name)) {
            return Distribution::Histogram(Histogram::new(buckets));
        }
        if let Some(buckets) = &self.buckets {
            return Distribution::Histogram(Histogram::new(buckets));
        }
        Distribution::Summary(
            RollingSummary::new(self.bucket_count, self.bucket_duration),
            self.quantiles.clone(),
            0.0,
        )
    }
}

/// A Prometheus histogram with fixed, cumulative buckets.
#[derive(Debug)]
pub(super) struct Histogram {
    count: u64,
    bounds: Vec<f64>,
    buckets: Vec<u64>,
    sum: f64,
}

impl Histogram {
    /// Creates a histogram with the given upper bounds. Empty bounds are rejected by the builder.
    fn new(bounds: &[f64]) -> Self {
        Self { count: 0, bounds: bounds.to_vec(), buckets: vec![0; bounds.len()], sum: 0.0 }
    }

    pub(super) const fn sum(&self) -> f64 {
        self.sum
    }

    pub(super) const fn count(&self) -> u64 {
        self.count
    }

    /// Returns `(upper bound, cumulative count)` pairs.
    pub(super) fn buckets(&self) -> impl Iterator<Item = (f64, u64)> + '_ {
        self.bounds.iter().copied().zip(self.buckets.iter().copied())
    }

    fn record_many(&mut self, samples: impl Iterator<Item = f64>) {
        let mut bucketed = vec![0u64; self.buckets.len()];
        let mut sum = 0.0;
        let mut count = 0;
        for sample in samples {
            sum += sample;
            count += 1;
            if let Some(idx) = self.bounds.iter().position(|bound| sample <= *bound) {
                bucketed[idx] += 1;
            }
        }

        // Make the buckets cumulative to satisfy the "less than or equal to" semantics.
        for idx in 1..bucketed.len() {
            bucketed[idx] += bucketed[idx - 1];
        }
        for (bucket, local) in self.buckets.iter_mut().zip(bucketed) {
            *bucket += local;
        }
        self.sum += sum;
        self.count += count;
    }
}

/// `DDSketch` backed quantile summary.
#[derive(Clone)]
pub(super) struct Summary {
    sketch: DDSketch,
}

impl Summary {
    /// Relative accuracy, bin limit and minimum tracked value of upstream `Summary::with_defaults`.
    fn new() -> Self {
        Self { sketch: DDSketch::new(Config::new(0.0001, 32_768, 1.0e-9)) }
    }

    fn add(&mut self, value: f64) {
        if value.is_infinite() {
            return;
        }
        self.sketch.add(value);
    }

    fn merge(&mut self, other: &Self) {
        // Merging only fails for sketches with different configs, and all sketches use `new`.
        self.sketch.merge(&other.sketch).expect("summary configs are identical");
    }

    /// Returns the estimated value at quantile `q`, or `None` if empty or `q` is out of range.
    pub(super) fn quantile(&self, q: f64) -> Option<f64> {
        if !(0.0..=1.0).contains(&q) || self.sketch.count() == 0 {
            return None;
        }
        self.sketch.quantile(q).ok().flatten()
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.sketch.count()
    }

    #[cfg(test)]
    fn min(&self) -> f64 {
        self.sketch.min().unwrap_or(f64::INFINITY)
    }

    #[cfg(test)]
    fn max(&self) -> f64 {
        self.sketch.max().unwrap_or(f64::NEG_INFINITY)
    }
}

impl std::fmt::Debug for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Summary").finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct Bucket {
    begin: Instant,
    summary: Summary,
}

/// Manages a list of [`Summary`] buckets so that old samples expire.
#[derive(Debug)]
pub(super) struct RollingSummary {
    /// Latest bucket first. Buckets stay aligned to the first bucket's start and
    /// `bucket_duration`, so the list may have gaps.
    buckets: Vec<Bucket>,
    max_buckets: usize,
    bucket_duration: Duration,
    /// Maximum age of a bucket that is still kept.
    max_bucket_duration: Duration,
    /// Total samples since creation, never reset by expiry.
    count: u64,
}

impl RollingSummary {
    /// Stores quantiles over `buckets * bucket_duration`.
    fn new(buckets: NonZeroU32, bucket_duration: Duration) -> Self {
        assert!(!bucket_duration.is_zero(), "bucket duration must be non-zero");
        let max_buckets = buckets.get() as usize;
        Self {
            buckets: Vec::with_capacity(max_buckets),
            max_buckets,
            bucket_duration,
            max_bucket_duration: bucket_duration * buckets.get(),
            count: 0,
        }
    }

    /// Adds a sample observed at `now`, expiring buckets that fell out of the window.
    fn add(&mut self, value: f64, now: Instant) {
        // The count is incremented even if this value is too old to be saved in any bucket.
        self.count += 1;

        for bucket in &mut self.buckets {
            let end = bucket.begin + self.bucket_duration;

            // This value belongs in a future bucket.
            if now > end {
                break;
            }

            if now >= bucket.begin && now < end {
                bucket.summary.add(value);
                return;
            }
        }

        if let Some(cutoff) = now.checked_sub(self.max_bucket_duration) {
            self.buckets.retain(|b| b.begin > cutoff);
        }

        let mut summary = Summary::new();
        summary.add(value);

        let Some(reftime) = self.buckets.first().map(|b| b.begin) else {
            self.buckets.push(Bucket { begin: now, summary });
            return;
        };

        // Other buckets are created at an offset of the first bucket's start. Values older than
        // the first bucket only bump the count.
        if now > reftime {
            let mut begin = reftime + self.bucket_duration;
            let mut end = begin + self.bucket_duration;
            while now < begin || now >= end {
                begin += self.bucket_duration;
                end += self.bucket_duration;
            }

            self.buckets.truncate(self.max_buckets - 1);
            self.buckets.insert(0, Bucket { begin, summary });
        }
    }

    /// Returns a merged [`Summary`] of all buckets still valid at `now`.
    ///
    /// The returned summary only counts samples in the window; use [`Self::count`] for the total.
    pub(super) fn snapshot(&self, now: Instant) -> Summary {
        let cutoff = now.checked_sub(self.max_bucket_duration);
        let mut acc = Summary::new();
        for bucket in &self.buckets {
            if cutoff.is_none_or(|cutoff| bucket.begin > cutoff) {
                acc.merge(&bucket.summary);
            }
        }
        acc
    }

    /// Total number of samples seen so far.
    pub(super) const fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> Instant {
        let (clock, mock) = quanta::Clock::mock();
        mock.increment(Duration::from_secs(secs));
        clock.now()
    }

    fn default_summary() -> RollingSummary {
        RollingSummary::new(DEFAULT_SUMMARY_BUCKET_COUNT, DEFAULT_SUMMARY_BUCKET_DURATION)
    }

    #[test]
    fn empty_snapshot() {
        let summary = default_summary();
        assert_eq!(summary.count(), 0);
        let snapshot = summary.snapshot(at(0));
        assert_eq!(snapshot.count(), 0);
        assert_eq!(snapshot.min(), f64::INFINITY);
        assert_eq!(snapshot.max(), f64::NEG_INFINITY);
        assert_eq!(snapshot.quantile(0.5), None);
    }

    #[test]
    fn snapshot_merges_buckets() {
        let mut summary = default_summary();
        summary.add(42.0, at(3600));
        summary.add(42.0, at(3620));
        summary.add(42.0, at(3640));

        let snapshot = summary.snapshot(at(3640));
        assert_eq!(snapshot.min(), 42.0);
        assert_eq!(snapshot.max(), 42.0);
        // 42 +/- (42 * 0.0001)
        assert!(Some(41.9958) < snapshot.quantile(0.5));
        assert!(Some(42.0042) > snapshot.quantile(0.5));
    }

    #[test]
    fn add_new_head() {
        let mut summary = default_summary();
        summary.add(42.0, at(3600));
        assert_eq!(summary.buckets.len(), 1);
        summary.add(42.0, at(3620));
        assert_eq!(summary.buckets.len(), 2);
    }

    #[test]
    fn truncate_old_buckets() {
        let mut summary = default_summary();
        summary.add(42.0, at(3600));
        for i in 1..=3 {
            summary.add(42.0, at(3600 + 20 * i));
        }
        assert_eq!(summary.buckets.len(), 3);
        assert_eq!(summary.count(), 4);
    }

    #[test]
    fn expired_samples_leave_window_but_keep_count() {
        let mut summary = default_summary();
        summary.add(1.0, at(3600));
        summary.add(100.0, at(3700));

        let snapshot = summary.snapshot(at(3700));
        assert_eq!(snapshot.count(), 1);
        assert_eq!(snapshot.min(), 100.0);
        assert_eq!(summary.count(), 2);

        // Nothing remains in the window once the newest bucket expires too.
        assert_eq!(summary.snapshot(at(3800)).count(), 0);
    }

    #[test]
    fn add_value_ts_before_first_bucket() {
        let mut summary = RollingSummary::new(NonZeroU32::new(2).unwrap(), Duration::from_secs(5));
        summary.add(42.0, at(4));
        assert_eq!(summary.buckets.len(), 1);
        assert_eq!(summary.count(), 1);

        // The first bucket begins at 4s, so a sample at 3s is counted but not stored.
        summary.add(43.0, at(3));
        assert_eq!(summary.buckets.len(), 1);
        assert_eq!(summary.count(), 2);
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let mut hist = Histogram::new(&[1.0, 2.0, 5.0]);
        hist.record_many([0.5, 1.0, 1.5, 3.0, 10.0].into_iter());
        assert_eq!(hist.buckets().collect::<Vec<_>>(), vec![(1.0, 2), (2.0, 3), (5.0, 4)]);
        assert_eq!(hist.count(), 5);
        assert_eq!(hist.sum(), 16.0);
    }

    #[test]
    fn bucket_overrides_take_precedence() {
        let builder = DistributionBuilder::new(
            vec![0.5],
            Some(vec![1.0]),
            DEFAULT_SUMMARY_BUCKET_DURATION,
            DEFAULT_SUMMARY_BUCKET_COUNT,
            vec![
                (Matcher::Suffix("_seconds".to_string()), vec![2.0, 3.0]),
                (Matcher::Full("foo_seconds".to_string()), vec![4.0]),
            ],
        );
        // `Full` sorts before `Suffix`.
        let Distribution::Histogram(hist) = builder.get_distribution("foo_seconds") else {
            panic!("expected histogram")
        };
        assert_eq!(hist.bounds, vec![4.0]);
        let Distribution::Histogram(hist) = builder.get_distribution("bar_seconds") else {
            panic!("expected histogram")
        };
        assert_eq!(hist.bounds, vec![2.0, 3.0]);
        let Distribution::Histogram(hist) = builder.get_distribution("bar") else {
            panic!("expected histogram")
        };
        assert_eq!(hist.bounds, vec![1.0]);
    }
}
