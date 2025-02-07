use alloc::sync::Arc;
use core::{error::Error, fmt};

/// A thread-safe cloneable wrapper for any error type.
#[derive(Clone)]
pub struct AnyError {
    inner: Arc<dyn Error + Send + Sync + 'static>,
}

impl AnyError {
    /// Creates a new `AnyError` wrapping the given error value.
    pub fn new<E>(error: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self { inner: Arc::new(error) }
    }

    /// Returns a reference to the underlying error value.
    pub fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.inner.as_ref()
    }
}

impl fmt::Debug for AnyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl fmt::Display for AnyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl Error for AnyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}
