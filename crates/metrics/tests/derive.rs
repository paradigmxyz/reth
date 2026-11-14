//! Recorder-backed coverage of the native and fuzz metrics derive APIs.

use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, IntoLabels, Key, KeyName, Label,
    Metadata, Recorder, SharedString, Unit,
};
use reth_metrics::Metrics;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Metrics with a fixed scope.
#[derive(Metrics)]
#[metrics(scope = "derive_test", separator = "_", labels = [("global", "label")])]
pub struct StaticMetrics {
    /// Counted events.
    #[metric(rename = "events", labels = [("field", "label")])]
    counter: Counter,
    /// Current value.
    gauge: Gauge,
    #[metric(describe = "Observed values.")]
    histogram: Histogram,
    #[metric(skip)]
    skipped: Vec<u8>,
}

/// Metrics with a caller-provided scope.
#[derive(Metrics)]
#[metrics(dynamic = true, labels = [("global", "label")])]
pub struct DynamicMetrics {
    /// Counted events.
    counter: Counter,
    /// Current value.
    gauge: Gauge,
    /// Observed values.
    histogram: Histogram,
    #[metric(skip)]
    skipped: Vec<u8>,
}

#[derive(Default)]
struct ObservedRecorder {
    registrations: AtomicUsize,
    descriptions: AtomicUsize,
    updates: Arc<ObservedUpdates>,
}

#[derive(Default)]
struct ObservedUpdates(AtomicUsize);

impl CounterFn for ObservedUpdates {
    fn increment(&self, _: u64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn absolute(&self, _: u64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl GaugeFn for ObservedUpdates {
    fn increment(&self, _: f64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn decrement(&self, _: f64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn set(&self, _: f64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl HistogramFn for ObservedUpdates {
    fn record(&self, _: f64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Recorder for ObservedRecorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {
        self.descriptions.fetch_add(1, Ordering::Relaxed);
    }

    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {
        self.descriptions.fetch_add(1, Ordering::Relaxed);
    }

    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {
        self.descriptions.fetch_add(1, Ordering::Relaxed);
    }

    fn register_counter(&self, _: &Key, _: &Metadata<'_>) -> Counter {
        self.registrations.fetch_add(1, Ordering::Relaxed);
        Counter::from_arc(self.updates.clone())
    }

    fn register_gauge(&self, _: &Key, _: &Metadata<'_>) -> Gauge {
        self.registrations.fetch_add(1, Ordering::Relaxed);
        Gauge::from_arc(self.updates.clone())
    }

    fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram {
        self.registrations.fetch_add(1, Ordering::Relaxed);
        Histogram::from_arc(self.updates.clone())
    }
}

struct ObservedLabels<'a>(&'a AtomicUsize);

impl IntoLabels for ObservedLabels<'_> {
    fn into_labels(self) -> Vec<Label> {
        self.0.fetch_add(1, Ordering::Relaxed);
        vec![Label::new("instance", "label")]
    }
}

#[test]
fn derives_obey_fuzz_mode_without_changing_the_recorder() {
    let recorder = ObservedRecorder::default();
    let label_conversions = AtomicUsize::new(0);
    metrics::with_local_recorder(&recorder, || {
        let first = StaticMetrics::default();
        let second = StaticMetrics::default();
        let labeled = StaticMetrics::new_with_labels(ObservedLabels(&label_conversions));
        StaticMetrics::describe();
        StaticMetrics::force_describe();
        assert_eq!(format!("{first:?}"), "StaticMetrics");
        for metric in [first, second, labeled] {
            assert_eq!(metric.skipped, Vec::<u8>::new());
            metric.counter.increment(1);
            metric.gauge.set(1.0);
            metric.histogram.record(1.0);
        }

        let first = DynamicMetrics::new("dynamic");
        let labeled =
            DynamicMetrics::new_with_labels("dynamic", ObservedLabels(&label_conversions));
        DynamicMetrics::describe("dynamic");
        assert_eq!(format!("{first:?}"), "DynamicMetrics");
        for metric in [first, labeled] {
            assert_eq!(metric.skipped, Vec::<u8>::new());
            metric.counter.increment(1);
            metric.gauge.set(1.0);
            metric.histogram.record(1.0);
        }
    });

    let counts = (
        recorder.registrations.load(Ordering::Relaxed),
        recorder.descriptions.load(Ordering::Relaxed),
        recorder.updates.0.load(Ordering::Relaxed),
        label_conversions.load(Ordering::Relaxed),
    );
    assert_eq!(counts, if cfg!(feature = "fuzz") { (0, 0, 0, 0) } else { (12, 15, 15, 2) });

    // A derived constructor must not replace or disable the caller's recorder.
    metrics::with_local_recorder(&recorder, || {
        metrics::counter!("direct_metric").increment(1);
    });
    assert_eq!(recorder.registrations.load(Ordering::Relaxed), counts.0 + 1);
    assert_eq!(recorder.updates.0.load(Ordering::Relaxed), counts.2 + 1);
}
