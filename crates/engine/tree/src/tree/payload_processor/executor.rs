//! Executor for mixed I/O and CPU workloads.

use reth_tasks::RUNTIME;
use tokio::{runtime::Handle, task::JoinHandle};

/// An executor for mixed I/O and CPU workloads.
///
/// This type uses the global [`RUNTIME`] tokio handle for spawning blocking tasks.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    handle: Handle,
}

impl Default for WorkloadExecutor {
    fn default() -> Self {
        Self { handle: RUNTIME.handle().clone() }
    }
}

impl WorkloadExecutor {
    /// Returns the handle to the tokio runtime
    pub(super) const fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Runs the provided function on an executor dedicated to blocking operations.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(func)
    }
}
