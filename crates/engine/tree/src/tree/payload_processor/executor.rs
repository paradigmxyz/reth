//! Executor for mixed I/O and CPU workloads.

use std::{sync::OnceLock, time::Duration};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    task::JoinHandle,
};

/// Sets the current thread's name for profiling visibility.
#[inline]
fn set_thread_name(name: &str) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: name is a valid string, prctl with PR_SET_NAME is safe
        unsafe {
            // PR_SET_NAME expects a null-terminated string, truncated to 16 bytes (including null)
            let mut buf = [0u8; 16];
            let len = name.len().min(15);
            buf[..len].copy_from_slice(&name.as_bytes()[..len]);
            libc::prctl(libc::PR_SET_NAME, buf.as_ptr());
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: name is a valid string
        unsafe {
            let c_name = std::ffi::CString::new(name).unwrap_or_default();
            libc::pthread_setname_np(c_name.as_ptr());
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = name;
    }
}

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

    /// Spawns a blocking task with a descriptive thread name for profiling.
    ///
    /// Sets the thread name at runtime, making it identifiable in profiling tools like Samply.
    /// Uses Tokio's blocking thread pool for efficiency.
    #[track_caller]
    pub fn spawn_blocking_named<F, R>(&self, name: &'static str, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.handle.spawn_blocking(move || {
            set_thread_name(name);
            func()
        })
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
