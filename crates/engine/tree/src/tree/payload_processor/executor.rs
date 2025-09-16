//! Executor for mixed I/O and CPU workloads.

use std::{sync::OnceLock, time::Duration};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    task::JoinHandle,
};

/// A simplified executor for I/O workloads.
///
/// This type provides a convenient interface for spawning blocking tasks
/// while managing runtime lifecycle efficiently.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    handle: Handle,
}

impl Default for WorkloadExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkloadExecutor {
    /// Creates a new executor that reuses existing runtime or creates its own.
    pub fn new() -> Self {
        Self { handle: get_or_create_runtime_handle() }
    }

    /// Returns the handle to the tokio runtime
    pub(super) const fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Shorthand for [`Runtime::spawn_blocking`]
    #[track_caller]
    pub(super) fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(func)
    }
}

/// Gets the current runtime handle or creates a new optimized runtime.
fn get_or_create_runtime_handle() -> Handle {
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
                // new block, and instead reuse the existing threads.
                .thread_keep_alive(Duration::from_secs(15))
                .build()
                .expect("Failed to create tokio runtime")
        });

        rt.handle().clone()
    })
}
