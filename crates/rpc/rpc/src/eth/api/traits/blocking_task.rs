//! Spawns a blocking task. Should be used for long-lived database reads.

use futures::Future;

use crate::eth::error::EthResult;

/// Executes code on a blocking thread.
pub trait SpawnBlocking {
    /// Executes the future on a new blocking task.
    ///
    /// Note: This is expected for futures that are dominated by blocking IO operations, for tracing
    /// or CPU bound operations in general use [`spawn_tracing`](Self::spawn_tracing).
    fn spawn_blocking_io<F, R>(&self, f: F) -> impl Future<Output = EthResult<R>> + Send
    where
        Self: Sized,
        F: FnOnce(Self) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Executes a blocking task on the tracing pool.
    ///
    /// Note: This is expected for futures that are predominantly CPU bound, as it uses `rayon`
    /// under the hood, for blocking IO futures use [`spawn_blocking`](Self::spawn_blocking_io). See
    /// <https://ryhl.io/blog/async-what-is-blocking/>.
    fn spawn_tracing<F, R>(&self, f: F) -> impl Future<Output = EthResult<R>>
    where
        Self: Sized,
        F: FnOnce(Self) -> EthResult<R> + Send + 'static,
        R: Send + 'static;
}
