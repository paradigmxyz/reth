//! Spawns a blocking task. CPU heavy tasks are executed with the `rayon` library. IO heavy tasks
//! are executed on the `tokio` runtime.

use futures::Future;
use reth_rpc_eth_types::EthApiError;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use std::sync::Arc;
use tokio::sync::{oneshot, AcquireError, OwnedSemaphorePermit, Semaphore};

use crate::EthApiTypes;

/// Helpers for spawning blocking operations.
///
/// Operations can be blocking because they require lots of CPU work and/or IO.
///
/// This differentiates between workloads that are primarily CPU bound and heavier in general (such
/// as tracing tasks) and tasks that have a more balanced profile (io and cpu), such as `eth_call`
/// and alike.
///
/// This provides access to semaphores that permit how many of those are permitted concurrently.
/// It's expected that tracing related tasks are configured with a lower threshold, because not only
/// are they CPU heavy but they can also accumulate more memory for the traces.
pub trait SpawnBlocking: EthApiTypes + Clone + Send + Sync + 'static {
    /// Returns a handle for spawning IO heavy blocking tasks.
    ///
    /// Runtime access in default trait method implementations.
    fn io_task_spawner(&self) -> impl TaskSpawner;

    /// Returns a handle for spawning __CPU heavy__ blocking tasks, such as tracing requests.
    ///
    /// Thread pool access in default trait method implementations.
    fn tracing_task_pool(&self) -> &BlockingTaskPool;

    /// Returns handle to semaphore for pool of CPU heavy blocking tasks.
    fn tracing_task_guard(&self) -> &BlockingTaskGuard;

    /// Returns handle to semaphore for blocking IO tasks.
    ///
    /// This semaphore is used to limit concurrent blocking IO operations like `eth_call`,
    /// `eth_estimateGas`, and similar methods that require EVM execution.
    fn blocking_io_task_guard(&self) -> &Arc<Semaphore>;

    /// Acquires a permit from the tracing task semaphore.
    ///
    /// This should be used for __CPU heavy__ operations like `debug_traceTransaction`,
    /// `debug_traceCall`, and similar tracing methods. These tasks are typically:
    /// - Primarily CPU bound with intensive computation
    /// - Can accumulate significant memory for trace results
    /// - Expected to have lower concurrency limits than general blocking IO tasks
    ///
    /// For blocking IO tasks like `eth_call` or `eth_estimateGas`, use
    /// [`acquire_owned_blocking_io`](Self::acquire_owned_blocking_io) instead.
    ///
    /// See also [`Semaphore::acquire_owned`](`tokio::sync::Semaphore::acquire_owned`).
    fn acquire_owned_tracing(
        &self,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.tracing_task_guard().clone().acquire_owned()
    }

    /// Acquires multiple permits from the tracing task semaphore.
    ///
    /// This should be used for particularly heavy tracing operations that require more resources
    /// than a standard trace. The permit count should reflect the expected resource consumption
    /// relative to a standard tracing operation.
    ///
    /// Like [`acquire_owned_tracing`](Self::acquire_owned_tracing), this is specifically for
    /// CPU-intensive tracing tasks, not general blocking IO operations.
    ///
    /// See also [`Semaphore::acquire_many_owned`](`tokio::sync::Semaphore::acquire_many_owned`).
    fn acquire_many_owned_tracing(
        &self,
        n: u32,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.tracing_task_guard().clone().acquire_many_owned(n)
    }

    /// Acquires a permit from the blocking IO request semaphore.
    ///
    /// This should be used for operations like `eth_call`, `eth_estimateGas`, and similar methods
    /// that require EVM execution and are spawned as blocking tasks.
    ///
    /// See also [`Semaphore::acquire_owned`](`tokio::sync::Semaphore::acquire_owned`).
    fn acquire_owned_blocking_io(
        &self,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.blocking_io_task_guard().clone().acquire_owned()
    }

    /// Acquires multiple permits from the blocking IO request semaphore.
    ///
    /// This should be used for operations that may require more resources than a single permit
    /// allows.
    ///
    /// See also [`Semaphore::acquire_many_owned`](`tokio::sync::Semaphore::acquire_many_owned`).
    fn acquire_many_owned_blocking_io(
        &self,
        n: u32,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.blocking_io_task_guard().clone().acquire_many_owned(n)
    }

    /// Acquires permits from the blocking IO request semaphore based on a calculated weight.
    ///
    /// The weight determines the maximum number of concurrent requests of this type that can run.
    /// For example, if the semaphore has 256 total permits and `weight=10`, then at most 10
    /// concurrent requests of this type are allowed.
    ///
    /// The permits acquired per request is calculated as `total_permits / weight`, with an
    /// adjustment: if this result is even, we add 1 to ensure that `weight - 1` permits are
    /// always available for other tasks, preventing complete semaphore exhaustion.
    ///
    /// This should be used to explicitly limit concurrent requests based on their expected
    /// resource consumption:
    ///
    /// - **Block range queries**: Higher weight for larger ranges (fewer concurrent requests)
    /// - **Complex calls**: Higher weight for expensive operations
    /// - **Batch operations**: Higher weight for larger batches
    /// - **Historical queries**: Higher weight for deeper history lookups
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // For a heavy request, use higher weight to limit concurrency
    /// let weight = 20; // Allow at most 20 concurrent requests of this type
    /// let _permit = self.acquire_weighted_blocking_io(weight).await?;
    /// ```
    ///
    /// This helps prevent resource exhaustion from concurrent expensive operations while allowing
    /// many cheap operations to run in parallel.
    ///
    /// See also [`Semaphore::acquire_many_owned`](`tokio::sync::Semaphore::acquire_many_owned`).
    fn acquire_weighted_blocking_io(
        &self,
        weight: u32,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        let guard = self.blocking_io_task_guard();
        let total_permits = guard.available_permits().max(1) as u32;
        let weight = weight.max(1);
        let mut permits_to_acquire = (total_permits / weight).max(1);

        // If total_permits divides evenly by weight, add 1 to ensure that when `weight`
        // concurrent requests are running, at least `weight - 1` permits remain available
        // for other tasks
        if total_permits.is_multiple_of(weight) {
            permits_to_acquire += 1;
        }

        guard.clone().acquire_many_owned(permits_to_acquire)
    }

    /// Executes the future on a new blocking task.
    ///
    /// Note: This is expected for futures that are dominated by blocking IO operations, for tracing
    /// or CPU bound operations in general use [`spawn_tracing`](Self::spawn_tracing).
    fn spawn_blocking_io<F, R>(&self, f: F) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        self.io_task_spawner().spawn_blocking_task(Box::pin(async move {
            let res = f(this);
            let _ = tx.send(res);
        }));

        async move { rx.await.map_err(|_| EthApiError::InternalEthError)? }
    }

    /// Executes the future on a new blocking task.
    ///
    /// Note: This is expected for futures that are dominated by blocking IO operations, for tracing
    /// or CPU bound operations in general use [`spawn_tracing`](Self::spawn_tracing).
    fn spawn_blocking_io_fut<F, R, Fut>(
        &self,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        Fut: Future<Output = Result<R, Self::Error>> + Send + 'static,
        F: FnOnce(Self) -> Fut + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        self.io_task_spawner().spawn_blocking_task(Box::pin(async move {
            let res = f(this).await;
            let _ = tx.send(res);
        }));

        async move { rx.await.map_err(|_| EthApiError::InternalEthError)? }
    }

    /// Executes a blocking task on the tracing pool.
    ///
    /// Note: This is expected for futures that are predominantly CPU bound, as it uses `rayon`
    /// under the hood, for blocking IO futures use
    /// [`spawn_blocking_task`](Self::spawn_blocking_io). See <https://ryhl.io/blog/async-what-is-blocking/>.
    fn spawn_tracing<F, R>(&self, f: F) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        let this = self.clone();
        let fut = self.tracing_task_pool().spawn(move || f(this));
        async move { fut.await.map_err(|_| EthApiError::InternalBlockingTaskError)? }
    }
}
