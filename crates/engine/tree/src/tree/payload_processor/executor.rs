use rayon::ThreadPool as RayonPool;
use std::sync::Arc;
use tokio::{runtime::Runtime, task::JoinHandle};

/// An executor for mixed I/O and CPU workloads.
#[derive(Debug, Clone)]
pub(super) struct WorkloadExecutor {
    inner: WorkloadExecutorInner,
}

impl WorkloadExecutor {
    pub(super) fn new(cpu_threads: usize) -> Self {
        // Create runtime for I/O operations
        let runtime = Arc::new(Runtime::new().unwrap());

        // Create Rayon thread pool for CPU work
        let rayon_pool =
            Arc::new(rayon::ThreadPoolBuilder::new().num_threads(cpu_threads).build().unwrap());

        Self { inner: WorkloadExecutorInner { runtime, rayon_pool } }
    }

    /// Returns access to the tokio runtime
    pub(super) fn runtime(&self) -> &Arc<Runtime> {
        &self.inner.runtime
    }

    /// Shorthand for [`Runtime::spawn_blocking`]
    #[track_caller]
    pub(super) fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.runtime().spawn_blocking(func)
    }

    /// Returns access to the rayon pool
    pub(super) fn rayon_pool(&self) -> &Arc<rayon::ThreadPool> {
        &self.inner.rayon_pool
    }
}

#[derive(Debug, Clone)]
struct WorkloadExecutorInner {
    // TODO: replace with main tokio handle instead or even task executor?
    runtime: Arc<Runtime>,
    rayon_pool: Arc<RayonPool>,
}
