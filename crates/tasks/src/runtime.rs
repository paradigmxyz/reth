//! Global runtime singleton for centralized management of async and parallel execution.
//!
//! This module provides [`GlobalRuntime`], a static singleton that manages:
//! - Tokio runtime (either owned or attached)
//! - [`TaskExecutor`] for spawning async tasks
//! - Rayon thread pool for CPU-bound work (with `rayon` feature)
//! - [`BlockingTaskPool`] and [`BlockingTaskGuard`] for rate-limited CPU tasks (with `rayon`
//!   feature)
//!
//! # Usage
//!
//! Initialize the runtime early in your application:
//!
//! ```ignore
//! use reth_tasks::{RuntimeConfig, TokioConfig, RayonConfig, RUNTIME};
//!
//! // Build and own a tokio runtime
//! let config = RuntimeConfig::default();
//! RUNTIME.init(config).expect("runtime already initialized");
//!
//! // Or attach to an existing runtime
//! let config = RuntimeConfig::with_existing_handle(handle);
//! RUNTIME.init(config).expect("runtime already initialized");
//! ```
//!
//! Then use the spawn functions anywhere:
//!
//! ```ignore
//! use reth_tasks::RUNTIME;
//!
//! // Spawn async tasks
//! RUNTIME.executor().spawn(async { /* ... */ });
//!
//! // Run CPU-bound work on rayon pool
//! RUNTIME.cpu_pool().spawn(|| expensive_computation());
//! ```

#[cfg(feature = "rayon")]
use crate::pool::{BlockingTaskGuard, BlockingTaskPool};
use crate::{TaskExecutor, TaskManager};
#[cfg(feature = "rayon")]
use std::thread::available_parallelism;
use std::{sync::OnceLock, time::Duration};
use tokio::runtime::{Handle, Runtime};
use tracing::debug;

/// Global runtime singleton.
///
/// Access via [`RUNTIME`].
pub static RUNTIME: GlobalRuntime = GlobalRuntime::new();

/// Default thread keep-alive duration for the tokio runtime.
///
/// Keeps threads alive for at least the block time (12 seconds) plus buffer.
/// This prevents the costly process of spawning new threads on every new block.
pub const DEFAULT_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Default reserved CPU cores for OS and other processes.
pub const DEFAULT_RESERVED_CPU_CORES: usize = 2;

/// Default maximum number of concurrent blocking tasks.
pub const DEFAULT_MAX_BLOCKING_TASKS: usize = 512;

/// Configuration for the tokio runtime.
#[derive(Debug, Clone)]
pub enum TokioConfig {
    /// Build and own a new tokio runtime.
    Owned {
        /// Number of worker threads. If `None`, uses tokio's default (number of CPU cores).
        worker_threads: Option<usize>,
        /// How long to keep worker threads alive when idle.
        thread_keep_alive: Duration,
        /// Thread name prefix.
        thread_name: &'static str,
    },
    /// Attach to an existing tokio runtime handle.
    ///
    /// Useful for embedding reth in another application or for tests.
    ExistingHandle(Handle),
}

impl Default for TokioConfig {
    fn default() -> Self {
        Self::Owned {
            worker_threads: None,
            thread_keep_alive: DEFAULT_THREAD_KEEP_ALIVE,
            thread_name: "tokio-rt",
        }
    }
}

impl TokioConfig {
    /// Create a config that attaches to an existing runtime handle.
    pub const fn existing_handle(handle: Handle) -> Self {
        Self::ExistingHandle(handle)
    }

    /// Create a config for an owned runtime with the specified number of worker threads.
    pub const fn with_worker_threads(worker_threads: usize) -> Self {
        Self::Owned {
            worker_threads: Some(worker_threads),
            thread_keep_alive: DEFAULT_THREAD_KEEP_ALIVE,
            thread_name: "tokio-rt",
        }
    }
}

/// Configuration for the rayon thread pool.
#[derive(Debug, Clone)]
#[cfg(feature = "rayon")]
pub struct RayonConfig {
    /// Number of threads for the CPU pool.
    /// If `None`, derived from available parallelism minus reserved cores.
    pub cpu_threads: Option<usize>,
    /// Number of CPU cores to reserve for OS and other processes.
    pub reserved_cpu_cores: usize,
    /// Thread name prefix for the rayon pool.
    pub thread_name_prefix: &'static str,
    /// Maximum number of concurrent blocking tasks (for `BlockingTaskGuard`).
    pub max_blocking_tasks: usize,
}

#[cfg(feature = "rayon")]
impl Default for RayonConfig {
    fn default() -> Self {
        Self {
            cpu_threads: None,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            thread_name_prefix: "reth-cpu",
            max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
        }
    }
}

#[cfg(feature = "rayon")]
impl RayonConfig {
    /// Create a config with the specified number of CPU threads.
    pub const fn with_cpu_threads(cpu_threads: usize) -> Self {
        Self {
            cpu_threads: Some(cpu_threads),
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            thread_name_prefix: "reth-cpu",
            max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
        }
    }

    /// Set the number of reserved CPU cores.
    pub const fn with_reserved_cpu_cores(mut self, reserved_cpu_cores: usize) -> Self {
        self.reserved_cpu_cores = reserved_cpu_cores;
        self
    }

    /// Set the maximum number of concurrent blocking tasks.
    pub const fn with_max_blocking_tasks(mut self, max_blocking_tasks: usize) -> Self {
        self.max_blocking_tasks = max_blocking_tasks;
        self
    }

    /// Compute the number of threads for the rayon pool.
    fn compute_thread_count(&self) -> usize {
        self.cpu_threads.unwrap_or_else(|| {
            available_parallelism()
                .map_or(1, |num| num.get().saturating_sub(self.reserved_cpu_cores).max(1))
        })
    }
}

/// Configuration for initializing the [`GlobalRuntime`].
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    /// Tokio runtime configuration.
    pub tokio: TokioConfig,
    /// Rayon thread pool configuration.
    #[cfg(feature = "rayon")]
    pub rayon: RayonConfig,
}

impl RuntimeConfig {
    /// Create a config that attaches to an existing tokio runtime handle.
    pub fn with_existing_handle(handle: Handle) -> Self {
        Self {
            tokio: TokioConfig::ExistingHandle(handle),
            #[cfg(feature = "rayon")]
            rayon: RayonConfig::default(),
        }
    }

    /// Set the tokio configuration.
    pub fn with_tokio(mut self, tokio: TokioConfig) -> Self {
        self.tokio = tokio;
        self
    }

    /// Set the rayon configuration.
    #[cfg(feature = "rayon")]
    pub const fn with_rayon(mut self, rayon: RayonConfig) -> Self {
        self.rayon = rayon;
        self
    }
}

/// Error returned when [`GlobalRuntime::init`] fails.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeInitError {
    /// The runtime has already been initialized.
    #[error("GlobalRuntime has already been initialized")]
    AlreadyInitialized,
    /// Failed to build the tokio runtime.
    #[error("Failed to build tokio runtime: {0}")]
    TokioBuild(#[from] std::io::Error),
    /// Failed to build the rayon thread pool.
    #[cfg(feature = "rayon")]
    #[error("Failed to build rayon thread pool: {0}")]
    RayonBuild(#[from] rayon::ThreadPoolBuildError),
}

/// Global runtime singleton that manages async and parallel execution resources.
///
/// This struct provides centralized access to:
/// - Tokio runtime/handle for async task execution
/// - [`TaskExecutor`] for spawning tasks with panic monitoring
/// - Rayon thread pool for CPU-bound parallel work
/// - [`BlockingTaskPool`] for async-friendly CPU tasks
/// - [`BlockingTaskGuard`] for rate-limiting expensive operations
///
/// All fields are lazily initialized via [`OnceLock`] and are private.
/// Use the getter methods to access resources after calling [`GlobalRuntime::init`].
pub struct GlobalRuntime {
    /// The stored configuration.
    config: OnceLock<RuntimeConfig>,
    /// The owned tokio runtime (if we built it).
    runtime: OnceLock<Runtime>,
    /// The tokio handle (always set after init).
    handle: OnceLock<Handle>,
    /// The task executor.
    executor: OnceLock<TaskExecutor>,
    /// The rayon CPU thread pool.
    #[cfg(feature = "rayon")]
    cpu_pool: OnceLock<rayon::ThreadPool>,
    /// The blocking task pool (wraps `cpu_pool` for async-friendly spawning).
    #[cfg(feature = "rayon")]
    blocking_pool: OnceLock<BlockingTaskPool>,
    /// Rate limiter for expensive CPU tasks.
    #[cfg(feature = "rayon")]
    blocking_guard: OnceLock<BlockingTaskGuard>,
}

impl GlobalRuntime {
    /// Create a new uninitialized `GlobalRuntime`.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            config: OnceLock::new(),
            runtime: OnceLock::new(),
            handle: OnceLock::new(),
            executor: OnceLock::new(),
            #[cfg(feature = "rayon")]
            cpu_pool: OnceLock::new(),
            #[cfg(feature = "rayon")]
            blocking_pool: OnceLock::new(),
            #[cfg(feature = "rayon")]
            blocking_guard: OnceLock::new(),
        }
    }

    /// Initialize the global runtime with the given configuration.
    ///
    /// This must be called once before using any getter methods.
    /// Returns an error if already initialized or if building the runtime fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use reth_tasks::{RuntimeConfig, RUNTIME};
    ///
    /// let task_manager = RUNTIME.init(RuntimeConfig::default())?;
    /// ```
    pub fn init(&self, config: RuntimeConfig) -> Result<TaskManager, RuntimeInitError> {
        // Store config first
        if self.config.set(config.clone()).is_err() {
            return Err(RuntimeInitError::AlreadyInitialized);
        }

        // Initialize tokio
        let handle = match &config.tokio {
            TokioConfig::Owned { worker_threads, thread_keep_alive, thread_name } => {
                let mut builder = tokio::runtime::Builder::new_multi_thread();
                builder
                    .enable_all()
                    .thread_keep_alive(*thread_keep_alive)
                    .thread_name(*thread_name);

                if let Some(threads) = worker_threads {
                    builder.worker_threads(*threads);
                }

                let runtime = builder.build()?;
                let h = runtime.handle().clone();

                // Store the owned runtime
                let _ = self.runtime.set(runtime);
                h
            }
            TokioConfig::ExistingHandle(h) => h.clone(),
        };

        // Store the handle
        let _ = self.handle.set(handle.clone());

        // Create TaskManager which also sets the global executor.
        // The caller is responsible for driving the TaskManager (polling it).
        let task_manager = TaskManager::new(handle);
        let executor = task_manager.executor();

        // Store the executor
        let _ = self.executor.set(executor);

        // Initialize rayon pools
        #[cfg(feature = "rayon")]
        {
            let num_threads = config.rayon.compute_thread_count();
            let thread_name_prefix = config.rayon.thread_name_prefix;

            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .thread_name(move |i| format!("{thread_name_prefix}-{i}"))
                .build()?;

            // Create the blocking pool wrapper
            let blocking_pool = BlockingTaskPool::new(pool);

            // Store cpu_pool as a separate pool with same config
            let cpu_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .thread_name(move |i| format!("{thread_name_prefix}-cpu-{i}"))
                .build()?;

            let _ = self.cpu_pool.set(cpu_pool);
            let _ = self.blocking_pool.set(blocking_pool);
            let _ =
                self.blocking_guard.set(BlockingTaskGuard::new(config.rayon.max_blocking_tasks));

            debug!(
                num_threads,
                max_blocking_tasks = config.rayon.max_blocking_tasks,
                "Initialized rayon thread pools"
            );
        }

        debug!("GlobalRuntime initialized");
        Ok(task_manager)
    }

    /// Initialize from an existing tokio handle.
    ///
    /// Convenience method that creates a [`RuntimeConfig`] with the given handle.
    pub fn init_with_handle(&self, handle: Handle) -> Result<TaskManager, RuntimeInitError> {
        self.init(RuntimeConfig::with_existing_handle(handle))
    }

    /// Returns `true` if the runtime has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.config.get().is_some()
    }

    /// Get the stored configuration, if initialized.
    pub fn try_config(&self) -> Option<&RuntimeConfig> {
        self.config.get()
    }

    /// Get the tokio runtime handle.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized. Call [`GlobalRuntime::init`] first.
    pub fn handle(&self) -> &Handle {
        self.try_handle().expect("GlobalRuntime not initialized. Call RUNTIME.init() first.")
    }

    /// Get the tokio runtime handle, if initialized.
    pub fn try_handle(&self) -> Option<&Handle> {
        self.handle.get()
    }

    /// Get a clone of the [`TaskExecutor`].
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized. Call [`GlobalRuntime::init`] first.
    pub fn executor(&self) -> TaskExecutor {
        self.try_executor().expect("GlobalRuntime not initialized. Call RUNTIME.init() first.")
    }

    /// Get a clone of the [`TaskExecutor`], if initialized.
    pub fn try_executor(&self) -> Option<TaskExecutor> {
        self.executor.get().cloned()
    }

    /// Get a reference to the rayon CPU thread pool.
    ///
    /// This pool is suitable for CPU-bound parallel work like hashing, signature verification, etc.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized. Call [`GlobalRuntime::init`] first.
    #[cfg(feature = "rayon")]
    pub fn cpu_pool(&self) -> &rayon::ThreadPool {
        self.try_cpu_pool().expect("GlobalRuntime not initialized. Call RUNTIME.init() first.")
    }

    /// Get a reference to the rayon CPU thread pool, if initialized.
    #[cfg(feature = "rayon")]
    pub fn try_cpu_pool(&self) -> Option<&rayon::ThreadPool> {
        self.cpu_pool.get()
    }

    /// Get a reference to the [`BlockingTaskPool`].
    ///
    /// This provides async-friendly access to CPU-bound tasks with panic handling.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized. Call [`GlobalRuntime::init`] first.
    #[cfg(feature = "rayon")]
    pub fn blocking_pool(&self) -> &BlockingTaskPool {
        self.try_blocking_pool().expect("GlobalRuntime not initialized. Call RUNTIME.init() first.")
    }

    /// Get a reference to the [`BlockingTaskPool`], if initialized.
    #[cfg(feature = "rayon")]
    pub fn try_blocking_pool(&self) -> Option<&BlockingTaskPool> {
        self.blocking_pool.get()
    }

    /// Get a clone of the [`BlockingTaskGuard`].
    ///
    /// Use this to rate-limit expensive CPU operations like trace execution or proof generation.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized. Call [`GlobalRuntime::init`] first.
    #[cfg(feature = "rayon")]
    pub fn blocking_guard(&self) -> BlockingTaskGuard {
        self.try_blocking_guard()
            .expect("GlobalRuntime not initialized. Call RUNTIME.init() first.")
    }

    /// Get a clone of the [`BlockingTaskGuard`], if initialized.
    #[cfg(feature = "rayon")]
    pub fn try_blocking_guard(&self) -> Option<BlockingTaskGuard> {
        self.blocking_guard.get().cloned()
    }

    /// Run a closure on the CPU pool, blocking until completion.
    ///
    /// This is equivalent to `cpu_pool().install(f)`.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized.
    #[cfg(feature = "rayon")]
    pub fn install_cpu<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R + Send,
        R: Send,
    {
        self.cpu_pool().install(f)
    }

    /// Spawn a CPU-bound task and return an async handle.
    ///
    /// This is equivalent to `blocking_pool().spawn(f)`.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized.
    #[cfg(feature = "rayon")]
    pub fn spawn_cpu<F, R>(&self, f: F) -> crate::pool::BlockingTaskHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.blocking_pool().spawn(f)
    }
}

impl std::fmt::Debug for GlobalRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalRuntime")
            .field("initialized", &self.is_initialized())
            .field("has_owned_runtime", &self.runtime.get().is_some())
            .finish()
    }
}

// SAFETY: All fields are protected by OnceLock which is Sync.
unsafe impl Sync for GlobalRuntime {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_default() {
        let config = RuntimeConfig::default();
        assert!(matches!(config.tokio, TokioConfig::Owned { .. }));
    }

    #[test]
    fn test_runtime_config_existing_handle() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = RuntimeConfig::with_existing_handle(rt.handle().clone());
        assert!(matches!(config.tokio, TokioConfig::ExistingHandle(_)));
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn test_rayon_config_thread_count() {
        let config = RayonConfig::default();
        let count = config.compute_thread_count();
        assert!(count >= 1);

        let config = RayonConfig::with_cpu_threads(4);
        assert_eq!(config.compute_thread_count(), 4);
    }
}
