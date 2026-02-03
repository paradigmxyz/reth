//! Executor for mixed I/O and CPU workloads.

use reth_trie_parallel::root::get_tokio_runtime_handle;
use tokio::{runtime::Handle, task::JoinHandle};

/// An executor for mixed I/O and CPU workloads.
///
/// This type uses tokio to spawn blocking tasks and will reuse an existing tokio
/// runtime if available or create its own.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    inner: WorkloadExecutorInner,
}

impl Default for WorkloadExecutor {
    fn default() -> Self {
        Self { inner: WorkloadExecutorInner::new() }
    }
}

impl WorkloadExecutor {
    /// Returns the handle to the tokio runtime
    pub(super) const fn handle(&self) -> &Handle {
        &self.inner.handle
    }

    /// Shorthand for [`Runtime::spawn_blocking`]
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.handle.spawn_blocking(func)
    }
}

#[derive(Debug, Clone)]
struct WorkloadExecutorInner {
    handle: Handle,
}

impl WorkloadExecutorInner {
    fn new() -> Self {
        Self { handle: get_tokio_runtime_handle() }
    }
}
