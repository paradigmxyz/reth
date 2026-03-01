//! A lazily-resolved handle to a value computed on a background thread.

use std::sync::{Arc, OnceLock};
use tokio::sync::oneshot;

/// Handle to a value computed on a background thread.
///
/// The computation is spawned immediately on creation and runs concurrently.
/// The result is resolved on first access via [`Self::get`] and cached in a
/// [`OnceLock`] for subsequent calls.
///
/// This type is cheaply cloneable via internal [`Arc`].
///
/// Create via [`Runtime::spawn_blocking_named`](crate::Runtime::spawn_blocking_named).
#[derive(Clone)]
pub struct LazyHandle<T> {
    inner: Arc<LazyHandleInner<T>>,
}

struct LazyHandleInner<T> {
    /// Pending receiver, taken on first access.
    rx: std::sync::Mutex<Option<oneshot::Receiver<T>>>,
    /// Cached result after the first successful receive.
    value: OnceLock<T>,
}

impl<T: Send + 'static> LazyHandle<T> {
    /// Creates a new handle from a background task receiver.
    pub(crate) fn new(rx: oneshot::Receiver<T>) -> Self {
        Self {
            inner: Arc::new(LazyHandleInner {
                rx: std::sync::Mutex::new(Some(rx)),
                value: OnceLock::new(),
            }),
        }
    }

    /// Creates a handle that is already resolved with the given value.
    pub fn ready(value: T) -> Self {
        let inner =
            LazyHandleInner { rx: std::sync::Mutex::new(None), value: OnceLock::from(value) };
        Self { inner: Arc::new(inner) }
    }

    /// Blocks until the background task completes and returns a reference to the result.
    ///
    /// On the first call this awaits the receiver; subsequent calls return the cached value
    /// without blocking.
    ///
    /// # Panics
    ///
    /// Panics if the background task was dropped without producing a value.
    pub fn get(&self) -> &T {
        self.inner.value.get_or_init(|| {
            let rx = self
                .inner
                .rx
                .lock()
                .expect("lock poisoned")
                .take()
                .expect("LazyHandle receiver already taken without value being set");
            rx.blocking_recv().expect("LazyHandle task dropped without producing a value")
        })
    }

    /// Consumes the handle and returns the inner value if this is the only handle.
    ///
    /// Returns `Err(self)` if other clones exist, so the caller can fall back
    /// to a reference-based path (e.g. `clone_into_sorted` instead of `into_sorted`).
    ///
    /// Blocks if the background task hasn't completed yet.
    pub fn try_into_inner(self) -> Result<T, Self> {
        self.get();
        match Arc::try_unwrap(self.inner) {
            Ok(inner) => Ok(inner.value.into_inner().expect("value was just set by get()")),
            Err(arc) => Err(Self { inner: arc }),
        }
    }
}

impl<T: Send + std::fmt::Debug + 'static> std::fmt::Debug for LazyHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("LazyHandle");
        if let Some(value) = self.inner.value.get() {
            s.field("value", value);
        } else {
            s.field("value", &"<pending>");
        }
        s.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_handle_resolves() {
        let (tx, rx) = oneshot::channel();
        let handle = LazyHandle::new(rx);
        tx.send(42u64).unwrap();
        assert_eq!(*handle.get(), 42);
        // subsequent calls return cached value
        assert_eq!(*handle.get(), 42);
    }

    #[test]
    fn test_lazy_handle_clone_shares_value() {
        let (tx, rx) = oneshot::channel();
        let handle = LazyHandle::new(rx);
        let handle2 = handle.clone();
        tx.send(99u64).unwrap();
        assert_eq!(*handle.get(), 99);
        assert_eq!(*handle2.get(), 99);
    }

    #[test]
    fn test_lazy_handle_try_into_inner() {
        let (tx, rx) = oneshot::channel();
        let handle = LazyHandle::new(rx);
        tx.send(String::from("hello")).unwrap();
        assert_eq!(handle.try_into_inner().unwrap(), "hello");
    }

    #[test]
    fn test_lazy_handle_try_into_inner_returns_self_with_clone() {
        let (tx, rx) = oneshot::channel();
        let handle = LazyHandle::new(rx);
        let _clone = handle.clone();
        tx.send(String::from("hello")).unwrap();
        let handle = handle.try_into_inner().unwrap_err();
        assert_eq!(*handle.get(), "hello");
    }

    #[test]
    #[should_panic(expected = "LazyHandle task dropped without producing a value")]
    fn test_lazy_handle_panics_on_dropped_sender() {
        let (_tx, rx) = oneshot::channel::<u64>();
        let handle = LazyHandle::new(rx);
        drop(_tx);
        handle.get();
    }
}
