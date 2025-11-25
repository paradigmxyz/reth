//! Executor for mixed I/O and CPU workloads.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    task::JoinHandle,
};

/// An executor for mixed I/O and CPU workloads.
///
/// This type uses tokio to spawn blocking tasks and will reuse an existing tokio
/// runtime if available or create its own. Blocking workloads are dispatched to a dedicated
/// runtime so that Tokio worker threads used by async tasks are never held up by long-running
/// computations (for example when waiting on deferred trie work).
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    inner: Arc<WorkloadExecutorInner>,
}

impl Default for WorkloadExecutor {
    fn default() -> Self {
        Self { inner: Arc::new(WorkloadExecutorInner::new()) }
    }
}

impl WorkloadExecutor {
    /// Returns the handle to the tokio runtime
    pub(super) fn handle(&self) -> &Handle {
        &self.inner.handle
    }

    /// Shorthand for [`Runtime::spawn_blocking`] on the dedicated blocking runtime.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.blocking.spawn_blocking(func)
    }
}

#[derive(Debug)]
struct WorkloadExecutorInner {
    /// Handle to the main runtime (usually the node or test runtime).
    handle: Handle,
    /// Dedicated runtime used exclusively for blocking workloads to avoid starving Tokio workers.
    blocking: Arc<Runtime>,
}

impl WorkloadExecutorInner {
    fn new() -> Self {
        fn get_runtime_handle() -> Handle {
            Handle::try_current().unwrap_or_else(|_| {
                // Create a new runtime if no runtime is available
                static RT: OnceLock<Runtime> = OnceLock::new();

                let rt = RT.get_or_init(|| {
                    Builder::new_multi_thread()
                        .enable_all()
                        // Keep the threads alive for at least the block time, which is 12 seconds
                        // at the time of writing, plus a little extra.
                        //
                        // This is to prevent the costly process of spawning new threads on every
                        // new block, and instead reuse the existing
                        // threads.
                        .thread_keep_alive(Duration::from_secs(15))
                        .build()
                        .unwrap()
                });

                rt.handle().clone()
            })
        }

        Self { handle: get_runtime_handle(), blocking: blocking_runtime() }
    }
}

/// Returns a shared runtime dedicated to blocking tasks.
///
/// By isolating blocking work from the main Tokio runtime we avoid deadlocks when async worker
/// threads are occupied waiting for deferred trie data.
fn blocking_runtime() -> Arc<Runtime> {
    static BLOCKING_RT: OnceLock<Arc<Runtime>> = OnceLock::new();

    BLOCKING_RT
        .get_or_init(|| {
            let worker_threads =
                std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1).max(2);

            Arc::new(
                Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(worker_threads)
                    .max_blocking_threads(worker_threads.saturating_mul(4))
                    .thread_keep_alive(Duration::from_secs(15))
                    .thread_name("reth-blocking")
                    .build()
                    .expect("failed to build blocking runtime"),
            )
        })
        .clone()
}
