use std::{fmt::Debug, sync::Arc};

use crate::IntoF64;

/// A counter handler.
pub trait CounterFn {
    /// Increments the counter by the given amount.
    fn increment(&self, value: u64);

    /// Sets the counter to at least the given amount.
    ///
    /// This is intended to support use cases where multiple callers are attempting to synchronize
    /// this counter with an external counter that they have no control over.  As multiple callers
    /// may read that external counter, and attempt to set it here, there could be reordering issues
    /// where a caller attempts to set an older (smaller) value after the counter has been updated to
    /// the latest (larger) value.
    ///
    /// This method must cope with those cases.  An example of doing so atomically can be found in
    /// `AtomicCounter`.
    fn absolute(&self, value: u64);
}

/// A gauge handler.
pub trait GaugeFn {
    /// Increments the gauge by the given amount.
    fn increment(&self, value: f64);

    /// Decrements the gauge by the given amount.
    fn decrement(&self, value: f64);

    /// Sets the gauge to the given amount.
    fn set(&self, value: f64);
}

/// A histogram handler.
pub trait HistogramFn {
    /// Records a value into the histogram.
    fn record(&self, value: f64);

    /// Records a value into the histogram multiple times.
    fn record_many(&self, value: f64, count: usize) {
        for _ in 0..count {
            self.record(value);
        }
    }
}

/// A counter.
#[derive(Clone)]
#[must_use = "counters do nothing unless you use them"]
pub struct Counter {
    inner: Option<Arc<dyn CounterFn + Send + Sync>>,
}

impl Debug for Counter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Counter").finish_non_exhaustive()
    }
}

/// A gauge.
#[derive(Clone)]
#[must_use = "gauges do nothing unless you use them"]
pub struct Gauge {
    inner: Option<Arc<dyn GaugeFn + Send + Sync>>,
}

impl Debug for Gauge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gauge").finish_non_exhaustive()
    }
}

/// A histogram.
#[derive(Clone)]
#[must_use = "histograms do nothing unless you use them"]
pub struct Histogram {
    inner: Option<Arc<dyn HistogramFn + Send + Sync>>,
}

impl Debug for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Histogram").finish_non_exhaustive()
    }
}

impl Counter {
    /// Creates a no-op `Counter` which does nothing.
    ///
    /// Suitable when a handle must be provided that does nothing i.e. a no-op recorder or a layer
    /// that disables specific metrics, and so on.
    pub const fn noop() -> Self {
        Self { inner: None }
    }

    /// Creates a `Counter` based on a shared handler.
    pub fn from_arc<F: CounterFn + Send + Sync + 'static>(a: Arc<F>) -> Self {
        Self { inner: Some(a) }
    }

    /// Increments the counter.
    pub fn increment(&self, value: u64) {
        if let Some(c) = &self.inner {
            c.increment(value)
        }
    }

    /// Sets the counter to an absolute value.
    pub fn absolute(&self, value: u64) {
        if let Some(c) = &self.inner {
            c.absolute(value)
        }
    }
}

impl Gauge {
    /// Creates a no-op `Gauge` which does nothing.
    ///
    /// Suitable when a handle must be provided that does nothing i.e. a no-op recorder or a layer
    /// that disables specific metrics, and so on.
    pub const fn noop() -> Self {
        Self { inner: None }
    }

    /// Creates a `Gauge` based on a shared handler.
    pub fn from_arc<F: GaugeFn + Send + Sync + 'static>(a: Arc<F>) -> Self {
        Self { inner: Some(a) }
    }

    /// Increments the gauge.
    pub fn increment<T: IntoF64>(&self, value: T) {
        if let Some(g) = &self.inner {
            g.increment(value.into_f64())
        }
    }

    /// Decrements the gauge.
    pub fn decrement<T: IntoF64>(&self, value: T) {
        if let Some(g) = &self.inner {
            g.decrement(value.into_f64())
        }
    }

    /// Sets the gauge.
    pub fn set<T: IntoF64>(&self, value: T) {
        if let Some(g) = &self.inner {
            g.set(value.into_f64())
        }
    }
}

impl Histogram {
    /// Creates a no-op `Histogram` which does nothing.
    ///
    /// Suitable when a handle must be provided that does nothing i.e. a no-op recorder or a layer
    /// that disables specific metrics, and so on.
    pub const fn noop() -> Self {
        Self { inner: None }
    }

    /// Creates a `Histogram` based on a shared handler.
    pub fn from_arc<F: HistogramFn + Send + Sync + 'static>(a: Arc<F>) -> Self {
        Self { inner: Some(a) }
    }

    /// Records a value into the histogram.
    pub fn record<T: IntoF64>(&self, value: T) {
        if let Some(ref inner) = self.inner {
            inner.record(value.into_f64())
        }
    }

    /// Records a value into the histogram multiple times.
    pub fn record_many<T: IntoF64>(&self, value: T, count: usize) {
        if let Some(ref inner) = self.inner {
            inner.record_many(value.into_f64(), count)
        }
    }
}

impl<T> CounterFn for Arc<T>
where
    T: CounterFn,
{
    fn increment(&self, value: u64) {
        (**self).increment(value)
    }

    fn absolute(&self, value: u64) {
        (**self).absolute(value)
    }
}
impl<T> GaugeFn for Arc<T>
where
    T: GaugeFn,
{
    fn increment(&self, value: f64) {
        (**self).increment(value)
    }

    fn decrement(&self, value: f64) {
        (**self).decrement(value)
    }

    fn set(&self, value: f64) {
        (**self).set(value)
    }
}

impl<T> HistogramFn for Arc<T>
where
    T: HistogramFn,
{
    fn record(&self, value: f64) {
        (**self).record(value);
    }
}

impl<T> From<Arc<T>> for Counter
where
    T: CounterFn + Send + Sync + 'static,
{
    fn from(inner: Arc<T>) -> Self {
        Counter::from_arc(inner)
    }
}

impl<T> From<Arc<T>> for Gauge
where
    T: GaugeFn + Send + Sync + 'static,
{
    fn from(inner: Arc<T>) -> Self {
        Gauge::from_arc(inner)
    }
}

impl<T> From<Arc<T>> for Histogram
where
    T: HistogramFn + Send + Sync + 'static,
{
    fn from(inner: Arc<T>) -> Self {
        Histogram::from_arc(inner)
    }
}
