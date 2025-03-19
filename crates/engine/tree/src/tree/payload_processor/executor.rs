//! Executor for mixed I/O and CPU workloads.

use rayon::ThreadPool as RayonPool;
use std::sync::{Arc, OnceLock};
use tokio::{
    runtime::{Builder, Handle, Runtime},
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
        Self {
            inner: WorkloadExecutorInner::new(
                rayon::ThreadPoolBuilder::new().build().unwrap(),
                None,
            ),
        }
    }
}

impl WorkloadExecutor {
    /// Creates a new executor with the given number of threads for cpu bound work (rayon) and
    /// an optional number of worker threads for the tokio runtime.
    #[allow(unused)]
    pub(super) fn with_num_cpu_threads(cpu_threads: usize, worker_threads: Option<usize>) -> Self {
        Self {
            inner: WorkloadExecutorInner::new(
                rayon::ThreadPoolBuilder::new().num_threads(cpu_threads).build().unwrap(),
                worker_threads,
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
    /// Creates a new `WorkloadExecutorInner` with the given Rayon thread pool and an optional
    /// number of worker threads for the Tokio runtime.
    ///
    /// Note: The Tokio runtime is lazily initialized using a static `OnceLock`. This means the
    /// runtime is created only once, on the first call to this function, using the
    /// `worker_threads` value provided at that time. Subsequent calls will ignore any different
    /// `worker_threads` values and reuse the existing runtime. This is acceptable for our use
    /// case, as the executor is initialized only once in the engine.
    fn new(rayon_pool: rayon::ThreadPool, worker_threads: Option<usize>) -> Self {
        fn get_runtime_handle(worker_threads: Option<usize>) -> Handle {
            Handle::try_current().unwrap_or_else(|_| {
                // Create a new runtime if no runtime is available
                static RT: OnceLock<Runtime> = OnceLock::new();

                let rt = RT.get_or_init(|| match worker_threads {
                    Some(num_of_threads) => {
                        Builder::new_multi_thread().worker_threads(num_of_threads).build().unwrap()
                    }
                    None => Runtime::new().unwrap(),
                });

                rt.handle().clone()
            })
        }

        Self { handle: get_runtime_handle(worker_threads), rayon_pool: Arc::new(rayon_pool) }
    }
}
