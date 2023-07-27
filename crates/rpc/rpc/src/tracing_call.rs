//! Additional helpers for executing tracing calls

use std::{
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    thread,
};
use tokio::sync::{oneshot, AcquireError, OwnedSemaphorePermit, Semaphore};

/// RPC Tracing call guard semaphore.
///
/// This is used to restrict the number of concurrent RPC requests to tracing methods like
/// `debug_traceTransaction` because they can consume a lot of memory and CPU.
///
/// This types serves as an entry guard for the [TracingCallPool] and is used to rate limit parallel
/// tracing calls on the pool.
#[derive(Clone, Debug)]
pub struct TracingCallGuard(Arc<Semaphore>);

impl TracingCallGuard {
    /// Create a new `TracingCallGuard` with the given maximum number of tracing calls in parallel.
    pub fn new(max_tracing_requests: u32) -> Self {
        Self(Arc::new(Semaphore::new(max_tracing_requests as usize)))
    }

    /// See also [Semaphore::acquire_owned]
    pub async fn acquire_owned(self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.0.acquire_owned().await
    }

    /// See also [Semaphore::acquire_many_owned]
    pub async fn acquire_many_owned(self, n: u32) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.0.acquire_many_owned(n).await
    }
}

/// Used to execute tracing calls on a rayon threadpool from within a tokio runtime.
///
/// This is a dedicated threadpool for tracing calls which are CPU bound.
/// RPC calls that perform blocking IO (disk lookups) are not executed on this pool but on the tokio
/// runtime's blocking pool, which performs poorly with CPU bound tasks. Once the tokio blocking
/// pool is saturated it is converted into a queue, tracing calls could then interfere with the
/// queue and block other RPC calls.
///
/// See also [tokio-docs] for more information.
///
/// [tokio-docs]: https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code
#[derive(Clone, Debug)]
pub struct TracingCallPool {
    pool: Arc<rayon::ThreadPool>,
}

impl TracingCallPool {
    /// Create a new `TracingCallPool` with the given threadpool.
    pub fn new(pool: rayon::ThreadPool) -> Self {
        Self { pool: Arc::new(pool) }
    }

    /// Convenience function to start building a new threadpool.
    pub fn builder() -> rayon::ThreadPoolBuilder {
        rayon::ThreadPoolBuilder::new()
    }

    /// Convenience function to build a new threadpool with the default configuration.
    ///
    /// Uses [`rayon::ThreadPoolBuilder::build`](rayon::ThreadPoolBuilder::build) defaults but
    /// increases the stack size to 8MB.
    pub fn build() -> Result<Self, rayon::ThreadPoolBuildError> {
        Self::builder()
            // increase stack size, mostly for RPC calls that use the evm: <https://github.com/paradigmxyz/reth/issues/3056> and  <https://github.com/bluealloy/revm/issues/305>
            .stack_size(8 * 1024 * 1024)
            .build()
            .map(Self::new)
    }

    /// Asynchronous wrapper around Rayon's
    /// [`ThreadPool::spawn`](rayon::ThreadPool::spawn).
    ///
    /// Runs a function on the configured threadpool, returning a future that resolves with the
    /// function's return value.
    ///
    /// If the function panics, the future will resolve to an error.
    pub fn spawn<F, R>(&self, func: F) -> TracingCallHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        self.pool.spawn(move || {
            let _result = tx.send(catch_unwind(AssertUnwindSafe(func)));
        });

        TracingCallHandle { rx }
    }

    /// Asynchronous wrapper around Rayon's
    /// [`ThreadPool::spawn_fifo`](rayon::ThreadPool::spawn_fifo).
    ///
    /// Runs a function on the configured threadpool, returning a future that resolves with the
    /// function's return value.
    ///
    /// If the function panics, the future will resolve to an error.
    pub fn spawn_fifo<F, R>(&self, func: F) -> TracingCallHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        self.pool.spawn_fifo(move || {
            let _result = tx.send(catch_unwind(AssertUnwindSafe(func)));
        });

        TracingCallHandle { rx }
    }
}

/// Async handle for a blocking tracing task running in a Rayon thread pool.
///
/// ## Panics
///
/// If polled from outside a tokio runtime.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
#[pin_project::pin_project]
pub struct TracingCallHandle<T> {
    #[pin]
    pub(crate) rx: oneshot::Receiver<thread::Result<T>>,
}

impl<T> Future for TracingCallHandle<T> {
    type Output = thread::Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.project().rx.poll(cx)) {
            Ok(res) => Poll::Ready(res),
            Err(_) => Poll::Ready(Err(Box::<TokioTracingCallError>::default())),
        }
    }
}

/// An error returned when the Tokio channel is dropped while awaiting a result.
///
/// This should only happen
#[derive(Debug, Default, thiserror::Error)]
#[error("Tokio channel dropped while awaiting result")]
#[non_exhaustive]
pub struct TokioTracingCallError;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tracing_pool() {
        let pool = TracingCallPool::build().unwrap();
        let res = pool.spawn(move || 5);
        let res = res.await.unwrap();
        assert_eq!(res, 5);
    }

    #[tokio::test]
    async fn tracing_pool_panic() {
        let pool = TracingCallPool::build().unwrap();
        let res = pool.spawn(move || -> i32 {
            panic!();
        });
        let res = res.await;
        assert!(res.is_err());
    }
}
