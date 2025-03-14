//! Executor for mixed I/O and CPU workloads.

use rayon::ThreadPool as RayonPool;
use std::sync::{Arc, OnceLock};
use tokio::{
    runtime::{Handle, Runtime},
    task::JoinHandle,
};

/// An executor for mixed I/O and CPU workloads.
///
/// This type has access to its own rayon pool and uses tokio to spawn blocking tasks.
///
/// It will reuse an existing tokio runtime if available or create its own.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    inner: WorkloadExecutorInner,
}

impl Default for WorkloadExecutor {
    fn default() -> Self {
        Self { inner: WorkloadExecutorInner::new(rayon::ThreadPoolBuilder::new().build().unwrap()) }
    }
}

impl WorkloadExecutor {
    /// Creates a new executor with the given number of threads for cpu bound work (rayon).
    #[allow(unused)]
    pub(super) fn with_num_cpu_threads(cpu_threads: usize) -> Self {
        Self {
            inner: WorkloadExecutorInner::new(
                rayon::ThreadPoolBuilder::new().num_threads(cpu_threads).build().unwrap(),
            ),
        }
    }

    /// Returns the handle to the tokio runtime
    pub(super) fn handle(&self) -> &Handle {
        &self.inner.handle
    }

    /// Shorthand for [`Runtime::spawn_blocking`]
    #[track_caller]
    pub(super) fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.handle.spawn_blocking(func)
    }

    /// Returns access to the rayon pool
    pub(super) fn rayon_pool(&self) -> &Arc<rayon::ThreadPool> {
        &self.inner.rayon_pool
    }
}

#[derive(Debug, Clone)]
struct WorkloadExecutorInner {
    handle: Handle,
    rayon_pool: Arc<RayonPool>,
}

impl WorkloadExecutorInner {
    fn new(rayon_pool: rayon::ThreadPool) -> Self {
        fn get_runtime_handle() -> Handle {
            Handle::try_current().unwrap_or_else(|_| {
                // Create a new runtime if now runtime is available
                static RT: OnceLock<Runtime> = OnceLock::new();

                let rt = RT.get_or_init(|| Runtime::new().unwrap());

                rt.handle().clone()
            })
        }

        Self { handle: get_runtime_handle(), rayon_pool: Arc::new(rayon_pool) }
    }
}
