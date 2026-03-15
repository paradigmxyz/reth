use metrics::Histogram;
use std::time::Instant;
use tracing::Span;

/// Extension trait for [`Span`] that ties a metrics [`Histogram`] to the span's lifetime.
///
/// When the returned [`MeteredSpan`] is dropped, the elapsed time since its creation
/// is recorded into the histogram as seconds (`f64`).
///
/// This eliminates the common pattern of:
/// ```ignore
/// let start = Instant::now();
/// // ... do work ...
/// histogram.record(start.elapsed().as_secs_f64());
/// ```
///
/// # Example
///
/// ```ignore
/// use tracing::debug_span;
/// use reth_metrics::SpanExt;
///
/// // Without entering the span:
/// let _metered = debug_span!("my_operation")
///     .metered(histogram.clone());
///
/// // Or, enter the span immediately:
/// let _metered = debug_span!("my_operation")
///     .metered(histogram.clone())
///     .entered();
///
/// // ... do work ...
/// // duration is recorded when `_metered` drops
/// ```
pub trait SpanExt {
    /// Attach a [`Histogram`] to this span. The span's lifetime determines when the duration
    /// is recorded — no manual [`Instant::now()`] or `.elapsed()` needed.
    fn metered(self, histogram: Histogram) -> MeteredSpan;
}

impl SpanExt for Span {
    fn metered(self, histogram: Histogram) -> MeteredSpan {
        MeteredSpan { _span: self, start: Instant::now(), histogram: Some(histogram) }
    }
}

/// A span that records its lifetime duration into a [`Histogram`] on drop.
///
/// Created via [`SpanExt::metered`].
#[must_use = "the metered span is immediately dropped if not bound to a variable"]
pub struct MeteredSpan {
    _span: Span,
    start: Instant,
    histogram: Option<Histogram>,
}

impl MeteredSpan {
    /// Enter the span, returning an entered guard that keeps the span active.
    ///
    /// The metric is recorded when the [`EnteredMeteredSpan`] is dropped.
    pub fn entered(mut self) -> EnteredMeteredSpan {
        let entered = self._span.clone().entered();
        // Take the histogram so `MeteredSpan::drop` becomes a no-op —
        // `EnteredMeteredSpan` takes over metric recording responsibility.
        let histogram = self.histogram.take().expect("histogram is always Some before entered()");
        EnteredMeteredSpan { _entered: entered, start: self.start, histogram }
    }

    /// Execute a closure within the span, recording the duration into the histogram.
    ///
    /// This is the metered equivalent of [`Span::in_scope`].
    pub fn in_scope<F: FnOnce() -> R, R>(self, f: F) -> R {
        self._span.in_scope(f)
        // `self` is dropped here, recording the elapsed time into the histogram
    }
}

impl std::fmt::Debug for MeteredSpan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeteredSpan").field("span", &self._span).finish_non_exhaustive()
    }
}

impl Drop for MeteredSpan {
    fn drop(&mut self) {
        if let Some(histogram) = &self.histogram {
            histogram.record(self.start.elapsed().as_secs_f64());
        }
    }
}

/// An entered [`MeteredSpan`] that records its duration on drop.
///
/// Created via [`MeteredSpan::entered`]. This is the equivalent of
/// `span.entered()` but with automatic metric recording.
#[must_use = "the entered metered span is immediately dropped if not bound to a variable"]
pub struct EnteredMeteredSpan {
    _entered: tracing::span::EnteredSpan,
    start: Instant,
    histogram: Histogram,
}

impl std::fmt::Debug for EnteredMeteredSpan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnteredMeteredSpan").finish_non_exhaustive()
    }
}

impl Drop for EnteredMeteredSpan {
    fn drop(&mut self) {
        self.histogram.record(self.start.elapsed().as_secs_f64());
    }
}
