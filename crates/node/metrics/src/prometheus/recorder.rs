//! The [`metrics::Recorder`] implementation and its render handle.

use super::{
    bucket::AtomicBucket,
    distribution::{Distribution, DistributionBuilder},
    formatting::{
        render_labels, sanitize_metric_name, write_help_line, write_metric_line, write_type_line,
    },
    registry::Registry,
};
use metrics::{
    atomics::AtomicU64, Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName,
    Metadata, Recorder, SharedString, Unit,
};
use parking_lot::{Mutex, RwLock};
use quanta::Instant;
use std::{
    collections::HashMap,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

/// A Prometheus recorder.
///
/// Install it as the global recorder through [`crate::recorder::PrometheusRecorder`], and render
/// its metrics through a [`PrometheusHandle`].
#[derive(Debug)]
pub struct PrometheusRecorder {
    inner: Arc<Inner>,
}

impl PrometheusRecorder {
    pub(super) fn new(
        distribution_builder: DistributionBuilder,
        global_labels: Vec<(String, String)>,
        prefix: Option<&'static str>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                counters: Registry::new(),
                gauges: Registry::new(),
                histograms: Registry::new(),
                descriptions: RwLock::default(),
                distribution_builder,
                global_labels,
                prefix,
                render_size_hint: AtomicUsize::new(0),
            }),
        }
    }

    /// Returns a [`PrometheusHandle`] to this recorder.
    pub fn handle(&self) -> PrometheusHandle {
        PrometheusHandle { inner: self.inner.clone() }
    }
}

impl Recorder for PrometheusRecorder {
    fn describe_counter(&self, key_name: KeyName, _unit: Option<Unit>, description: SharedString) {
        self.inner.describe(&key_name, description);
    }

    fn describe_gauge(&self, key_name: KeyName, _unit: Option<Unit>, description: SharedString) {
        self.inner.describe(&key_name, description);
    }

    fn describe_histogram(
        &self,
        key_name: KeyName,
        _unit: Option<Unit>,
        description: SharedString,
    ) {
        self.inner.describe(&key_name, description);
    }

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        Counter::from_arc(self.inner.counters.get_or_create(key, || CounterEntry {
            value: AtomicU64::new(0),
            series: self.inner.series(key),
        }))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(self.inner.gauges.get_or_create(key, || GaugeEntry {
            value: AtomicU64::new(0),
            series: self.inner.series(key),
        }))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(self.inner.histograms.get_or_create(key, || {
            let series = self.inner.series(key);
            HistogramEntry {
                samples: AtomicBucket::new(),
                distribution: Mutex::new(
                    self.inner.distribution_builder.get_distribution(&series.name),
                ),
                series,
            }
        }))
    }
}

/// Handle for rendering metrics stored by a [`PrometheusRecorder`].
#[derive(Clone, Debug)]
pub struct PrometheusHandle {
    inner: Arc<Inner>,
}

impl PrometheusHandle {
    /// Renders all metrics in the Prometheus text exposition format.
    ///
    /// Series are grouped by metric name and sorted by name and labels.
    pub fn render(&self) -> String {
        self.inner.render()
    }

    /// Renders all metrics like [`Self::render`] and writes them to `output`.
    pub fn render_to_write(&self, output: &mut impl io::Write) -> io::Result<()> {
        output.write_all(self.inner.render().as_bytes())
    }

    /// Drains recorded histogram samples into their distributions.
    ///
    /// Samples are buffered until the next upkeep or render, so this must run periodically to
    /// bound memory between scrapes.
    pub fn run_upkeep(&self) {
        for histogram in self.inner.histograms.entries() {
            histogram.drain(&mut histogram.distribution.lock());
        }
    }
}

#[derive(Debug)]
struct Inner {
    counters: Registry<CounterEntry>,
    gauges: Registry<GaugeEntry>,
    histograms: Registry<HistogramEntry>,
    /// Descriptions keyed by sanitized metric name. The first description wins, while upstream
    /// keeps the last one described before its next upkeep.
    descriptions: RwLock<HashMap<String, SharedString>>,
    distribution_builder: DistributionBuilder,
    global_labels: Vec<(String, String)>,
    /// Prepended as `<prefix>.` to every metric name before sanitization.
    prefix: Option<&'static str>,
    /// Length of the previous render, used to size the next output buffer.
    render_size_hint: AtomicUsize,
}

impl Inner {
    /// Returns the sanitized, prefixed metric name for `name`.
    fn metric_name(&self, name: &str) -> String {
        match self.prefix {
            Some(prefix) => sanitize_metric_name(&format!("{prefix}.{name}")),
            None => sanitize_metric_name(name),
        }
    }

    fn describe(&self, key_name: &KeyName, description: SharedString) {
        let name = self.metric_name(key_name.as_str());
        if self.descriptions.read().contains_key(&name) {
            return;
        }
        self.descriptions.write().entry(name).or_insert(description);
    }

    /// Resolves the rendered name and labels of a new series.
    ///
    /// Key labels override global labels with the same key, keeping the global label's position.
    fn series(&self, key: &Key) -> Series {
        let mut labels = self
            .global_labels
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        for label in key.labels() {
            if let Some(existing) = labels.iter_mut().find(|(key, _)| *key == label.key()) {
                existing.1 = label.value();
            } else {
                labels.push((label.key(), label.value()));
            }
        }
        Series {
            name: self.metric_name(key.name()).into_boxed_str(),
            labels: render_labels(labels).into_boxed_str(),
        }
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(self.render_size_hint.load(Ordering::Relaxed));

        let mut counters = self.counters.entries();
        counters.sort_unstable_by(|a, b| a.series.cmp(&b.series));
        for group in counters.chunk_by(|a, b| a.series.name == b.series.name) {
            let name = &group[0].series.name;
            self.write_header(&mut out, name, "counter");
            for counter in group {
                let value = counter.value.load(Ordering::Acquire);
                write_metric_line(&mut out, name, None, &counter.series.labels, NO_LABEL, value);
            }
            out.push('\n');
        }

        let mut gauges = self.gauges.entries();
        gauges.sort_unstable_by(|a, b| a.series.cmp(&b.series));
        for group in gauges.chunk_by(|a, b| a.series.name == b.series.name) {
            let name = &group[0].series.name;
            self.write_header(&mut out, name, "gauge");
            for gauge in group {
                let value = f64::from_bits(gauge.value.load(Ordering::Acquire));
                write_metric_line(&mut out, name, None, &gauge.series.labels, NO_LABEL, value);
            }
            out.push('\n');
        }

        let now = Instant::now();
        let mut histograms = self.histograms.entries();
        histograms.sort_unstable_by(|a, b| a.series.cmp(&b.series));
        for group in histograms.chunk_by(|a, b| a.series.name == b.series.name) {
            let name = &group[0].series.name;
            for (idx, histogram) in group.iter().enumerate() {
                let mut distribution = histogram.distribution.lock();
                histogram.drain(&mut distribution);
                // Every series of a name resolves to the same distribution kind.
                if idx == 0 {
                    let metric_type = match &*distribution {
                        Distribution::Histogram(_) => "histogram",
                        Distribution::Summary(..) => "summary",
                    };
                    self.write_header(&mut out, name, metric_type);
                }
                write_distribution(&mut out, name, &histogram.series.labels, &distribution, now);
            }
            out.push('\n');
        }

        self.render_size_hint.store(out.len(), Ordering::Relaxed);
        out
    }

    fn write_header(&self, out: &mut String, name: &str, metric_type: &str) {
        if let Some(description) = self.descriptions.read().get(name) {
            write_help_line(out, name, description);
        }
        write_type_line(out, name, metric_type);
    }
}

/// Rendered identity of a series, ordered by name, then labels.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Series {
    /// Sanitized metric name, including the prefix.
    name: Box<str>,
    /// Sanitized `key="value"` pairs, including global labels.
    labels: Box<str>,
}

#[derive(Debug)]
struct CounterEntry {
    value: AtomicU64,
    series: Series,
}

impl CounterFn for CounterEntry {
    fn increment(&self, value: u64) {
        CounterFn::increment(&self.value, value);
    }

    fn absolute(&self, value: u64) {
        CounterFn::absolute(&self.value, value);
    }
}

#[derive(Debug)]
struct GaugeEntry {
    /// Bits of the `f64` value.
    value: AtomicU64,
    series: Series,
}

impl GaugeFn for GaugeEntry {
    fn increment(&self, value: f64) {
        GaugeFn::increment(&self.value, value);
    }

    fn decrement(&self, value: f64) {
        GaugeFn::decrement(&self.value, value);
    }

    fn set(&self, value: f64) {
        GaugeFn::set(&self.value, value);
    }
}

#[derive(Debug)]
struct HistogramEntry {
    /// Samples recorded since the last drain.
    ///
    /// A sample pushed into a block while a drain is already reading it can be dropped, as with
    /// upstream's `AtomicBucket`.
    samples: AtomicBucket<(f64, Instant)>,
    /// Aggregated samples, only locked by upkeep and render.
    distribution: Mutex<Distribution>,
    series: Series,
}

impl HistogramEntry {
    fn drain(&self, distribution: &mut Distribution) {
        self.samples.clear_with(|samples| distribution.record_samples(samples));
    }
}

impl HistogramFn for HistogramEntry {
    fn record(&self, value: f64) {
        self.samples.push((value, Instant::now()));
    }
}

/// Placeholder for lines without an additional label.
const NO_LABEL: Option<(&str, u8)> = None;

fn write_distribution(
    out: &mut String,
    name: &str,
    labels: &str,
    distribution: &Distribution,
    now: Instant,
) {
    let (sum, count) = match distribution {
        Distribution::Summary(summary, quantiles, sum) => {
            let snapshot = summary.snapshot(now);
            for &quantile in quantiles.iter() {
                let value = snapshot.quantile(quantile).unwrap_or(0.0);
                write_metric_line(out, name, None, labels, Some(("quantile", quantile)), value);
            }
            (*sum, summary.count())
        }
        Distribution::Histogram(histogram) => {
            for (le, count) in histogram.buckets() {
                write_metric_line(out, name, Some("bucket"), labels, Some(("le", le)), count);
            }
            let count = histogram.count();
            write_metric_line(out, name, Some("bucket"), labels, Some(("le", "+Inf")), count);
            (histogram.sum(), count)
        }
    };
    write_metric_line(out, name, Some("sum"), labels, NO_LABEL, sum);
    write_metric_line(out, name, Some("count"), labels, NO_LABEL, count);
}
