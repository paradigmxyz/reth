//! Executor for mixed I/O and CPU workloads.

use std::{sync::OnceLock, time::Duration};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    task::JoinHandle,
};

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

        Self { handle: get_runtime_handle() }
    }
}
