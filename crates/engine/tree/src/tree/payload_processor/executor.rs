//! Executor for mixed I/O and CPU workloads.

use reth_tasks::Runtime;
use tokio::task::JoinHandle;

/// An executor for mixed I/O and CPU workloads.
///
/// This type wraps the [`Runtime`] for spawning blocking tasks.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    runtime: Runtime,
}

impl WorkloadExecutor {
    /// Create a new executor with the given runtime.
    pub const fn new(runtime: Runtime) -> Self {
        Self { runtime }
    }

    /// Returns a reference to the underlying [`Runtime`].
    pub const fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Runs the provided function on an executor dedicated to blocking operations.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.runtime.spawn_blocking_fn(func)
    }
}
