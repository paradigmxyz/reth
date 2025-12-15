//! Thread-safe cancellation primitives for cooperative task cancellation.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A thread-safe cancellation token that can be shared across threads.
///
/// This token allows cooperative cancellation by providing a way to signal
/// cancellation and check cancellation status. The token can be cloned and
/// shared across multiple threads, with all clones sharing the same cancellation state.
///
/// # Example
///
/// ```
/// use reth_cli_util::cancellation::CancellationToken;
/// use std::{thread, time::Duration};
///
/// let token = CancellationToken::new();
/// let worker_token = token.clone();
///
/// let handle = thread::spawn(move || {
///     while !worker_token.is_cancelled() {
///         // Do work...
///         thread::sleep(Duration::from_millis(100));
///     }
/// });
///
/// // Cancel from main thread
/// token.cancel();
/// handle.join().unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a new cancellation token in the non-cancelled state.
    pub fn new() -> Self {
        Self { cancelled: Arc::new(AtomicBool::new(false)) }
    }

    /// Signals cancellation to all holders of this token and its clones.
    ///
    /// Once cancelled, the token cannot be reset. This operation is thread-safe
    /// and can be called multiple times without issue.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Checks whether cancellation has been requested.
    ///
    /// Returns `true` if [`cancel`](Self::cancel) has been called on this token
    /// or any of its clones.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Creates a guard that automatically cancels this token when dropped.
    ///
    /// This is useful for ensuring cancellation happens when a scope exits,
    /// either normally or via panic.
    ///
    /// # Example
    ///
    /// ```
    /// use reth_cli_util::cancellation::CancellationToken;
    ///
    /// let token = CancellationToken::new();
    /// {
    ///     let _guard = token.drop_guard();
    ///     assert!(!token.is_cancelled());
    ///     // Guard dropped here, triggering cancellation
    /// }
    /// assert!(token.is_cancelled());
    /// ```
    pub fn drop_guard(&self) -> CancellationGuard {
        CancellationGuard { token: self.clone() }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// A guard that cancels its associated [`CancellationToken`] when dropped.
///
/// Created by calling [`CancellationToken::drop_guard`]. When this guard is dropped,
/// it automatically calls [`cancel`](CancellationToken::cancel) on the token.
#[derive(Debug)]
pub struct CancellationGuard {
    token: CancellationToken,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        self.token.cancel();
    }
}
