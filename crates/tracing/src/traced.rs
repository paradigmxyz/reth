//! Propagates tracing context across channel boundaries.
//!
//! ```ignore
//! // send side
//! tx.send(Traced::new(payload));
//!
//! // receive side — span is re-entered automatically
//! msg.in_span(|payload| process(payload));
//! ```

use tracing::Span;

/// A value bundled with the [`tracing::Span`] that was active when it was created.
///
/// Access to the inner value is gated behind [`in_span`](Self::in_span) so the
/// caller cannot forget to re-enter the captured span.
pub struct Traced<T> {
    inner: T,
    span: Span,
}

impl<T: core::fmt::Debug> core::fmt::Debug for Traced<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Traced").field("inner", &self.inner).finish()
    }
}

impl<T: Clone> Clone for Traced<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), span: self.span.clone() }
    }
}

impl<T> Traced<T> {
    /// Captures [`Span::current()`] and wraps `value`.
    pub fn new(value: T) -> Self {
        Self { inner: value, span: Span::current() }
    }

    /// Wraps `value` with an explicit span.
    pub const fn with_span(value: T, span: Span) -> Self {
        Self { inner: value, span }
    }

    /// Enters the captured span, then calls `f` with a reference to the inner value.
    pub fn in_span<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let _guard = self.span.enter();
        f(&self.inner)
    }

    /// Enters the captured span, consumes `self`, and passes the inner value to `f`.
    pub fn take_in_span<F, R>(self, f: F) -> R
    where
        F: FnOnce(T) -> R,
    {
        let _guard = self.span.enter();
        f(self.inner)
    }

    /// Transforms the inner value while preserving the captured span.
    pub fn map<U, F>(self, f: F) -> Traced<U>
    where
        F: FnOnce(T) -> U,
    {
        Traced { inner: f(self.inner), span: self.span }
    }

    /// Enters the captured span and returns the inner value alongside a guard.
    ///
    /// The span stays active until the returned [`tracing::span::EnteredSpan`]
    /// is dropped, covering all processing in the caller's scope.
    pub fn enter_traced(self) -> (tracing::span::EnteredSpan, T) {
        (self.span.entered(), self.inner)
    }

    /// Returns a reference to the inner value **without** entering the captured span.
    ///
    /// Use only for display/logging where span context is irrelevant.
    /// Prefer [`in_span`](Self::in_span) for actual processing.
    pub const fn untraced(&self) -> &T {
        &self.inner
    }
}
