//! Spawns a blocking task. CPU heavy tasks are executed with the `rayon` library. IO heavy tasks
//! are executed on the `tokio` runtime.

use futures::Future;
use reth_rpc_eth_types::EthApiError;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use tokio::sync::{oneshot, AcquireError, OwnedSemaphorePermit};

use crate::EthApiTypes;

/// Executes code on a blocking thread.
pub trait SpawnBlocking: EthApiTypes + Clone + Send + Sync + 'static {
    /// Returns a handle for spawning IO heavy blocking tasks.
    ///
    /// Runtime access in default trait method implementations.
    fn io_task_spawner(&self) -> impl TaskSpawner;

    /// Returns a handle for spawning CPU heavy blocking tasks.
    ///
    /// Thread pool access in default trait method implementations.
    fn tracing_task_pool(&self) -> &BlockingTaskPool;

    /// Returns handle to semaphore for pool of CPU heavy blocking tasks.
    fn tracing_task_guard(&self) -> &BlockingTaskGuard;

    /// See also [`Semaphore::acquire_owned`](`tokio::sync::Semaphore::acquire_owned`).
    fn acquire_owned(
        &self,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.tracing_task_guard().clone().acquire_owned()
    }

    /// See also  [`Semaphore::acquire_many_owned`](`tokio::sync::Semaphore::acquire_many_owned`).
    fn acquire_many_owned(
        &self,
        n: u32,
    ) -> impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send {
        self.tracing_task_guard().clone().acquire_many_owned(n)
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
        self.io_task_spawner().spawn_blocking(Box::pin(async move {
            let res = async move { f(this) }.await;
            let _ = tx.send(res);
        }));

        async move { rx.await.map_err(|_| EthApiError::InternalEthError)? }
    }

    /// Executes a blocking task on the tracing pool.
    ///
    /// Note: This is expected for futures that are predominantly CPU bound, as it uses `rayon`
    /// under the hood, for blocking IO futures use [`spawn_blocking`](Self::spawn_blocking_io). See
    /// <https://ryhl.io/blog/async-what-is-blocking/>.
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
