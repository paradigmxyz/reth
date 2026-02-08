//! Global runtime singleton for centralized management of async and parallel execution.
//!
//! This module provides [`GlobalRuntime`], a static singleton that manages:
//! - Tokio runtime (either owned or attached)
//! - [`TaskExecutor`] for spawning async tasks
//! - Dedicated rayon thread pools for different workloads (with `rayon` feature)
//! - [`BlockingTaskGuard`] for rate-limiting expensive operations (with `rayon` feature)
//!
//! # Usage
//!
//! Initialize the runtime early in your application:
//!
//! ```ignore
//! use reth_tasks::{RuntimeConfig, RUNTIME};
//!
//! let task_manager = RUNTIME.init(RuntimeConfig::default())?;
//! ```
//!
//! Then use the getters anywhere:
//!
//! ```ignore
//! use reth_tasks::RUNTIME;
//!
//! // Spawn async tasks
//! RUNTIME.executor().spawn(async { /* ... */ });
//!
//! // Run CPU-bound work on the general rayon pool
//! RUNTIME.cpu_pool().install(|| expensive_computation());
//!
//! // Run trie proof work on the dedicated trie pool
//! RUNTIME.trie_pool().spawn(|| compute_proof());
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
pub static RUNTIME: GlobalRuntime = GlobalRuntime::new();

/// Default thread keep-alive duration for the tokio runtime.
pub const DEFAULT_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Default reserved CPU cores for OS and other processes.
pub const DEFAULT_RESERVED_CPU_CORES: usize = 2;

/// Default number of threads for the storage I/O pool.
pub const DEFAULT_STORAGE_POOL_THREADS: usize = 16;

/// Default maximum number of concurrent blocking tasks (for RPC tracing guard).
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

/// Configuration for the rayon thread pools.
#[derive(Debug, Clone)]
#[cfg(feature = "rayon")]
pub struct RayonConfig {
    /// Number of threads for the general CPU pool.
    /// If `None`, derived from available parallelism minus reserved cores.
    pub cpu_threads: Option<usize>,
    /// Number of CPU cores to reserve for OS and other processes.
    pub reserved_cpu_cores: usize,
    /// Number of threads for the RPC blocking pool (trace calls, `eth_getProof`, etc.).
    /// If `None`, uses the same as `cpu_threads`.
    pub rpc_threads: Option<usize>,
    /// Number of threads for the trie proof computation pool.
    /// If `None`, uses the same as `cpu_threads`.
    pub trie_threads: Option<usize>,
    /// Number of threads for the storage I/O pool (static file, `RocksDB` writes in
    /// `save_blocks`). If `None`, uses [`DEFAULT_STORAGE_POOL_THREADS`].
    pub storage_threads: Option<usize>,
    /// Maximum number of concurrent blocking tasks for the RPC guard semaphore.
    pub max_blocking_tasks: usize,
}

#[cfg(feature = "rayon")]
impl Default for RayonConfig {
    fn default() -> Self {
        Self {
            cpu_threads: None,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            rpc_threads: None,
            trie_threads: None,
            storage_threads: None,
            max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
        }
    }
}

#[cfg(feature = "rayon")]
impl RayonConfig {
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

    /// Set the number of threads for the RPC blocking pool.
    pub const fn with_rpc_threads(mut self, rpc_threads: usize) -> Self {
        self.rpc_threads = Some(rpc_threads);
        self
    }

    /// Set the number of threads for the trie proof pool.
    pub const fn with_trie_threads(mut self, trie_threads: usize) -> Self {
        self.trie_threads = Some(trie_threads);
        self
    }

    /// Set the number of threads for the storage I/O pool.
    pub const fn with_storage_threads(mut self, storage_threads: usize) -> Self {
        self.storage_threads = Some(storage_threads);
        self
    }

    /// Compute the default number of threads based on available parallelism.
    fn default_thread_count(&self) -> usize {
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
    #[cfg_attr(not(feature = "rayon"), allow(clippy::missing_const_for_fn))]
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
    /// Failed to build a rayon thread pool.
    #[cfg(feature = "rayon")]
    #[error("Failed to build rayon thread pool: {0}")]
    RayonBuild(#[from] rayon::ThreadPoolBuildError),
}

/// Returns a fallback tokio [`Handle`] if one is available in the current context (e.g. in tests),
/// otherwise panics.
fn fallback_handle() -> Handle {
    match Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => uninitialized_panic(),
    }
}

/// Returns a fallback [`TaskExecutor`] if a tokio runtime is available in the current context
/// (e.g. in tests), otherwise panics.
fn fallback_executor() -> TaskExecutor {
    TaskManager::new(fallback_handle()).executor()
}

/// Returns a fallback rayon pool with the given name prefix. Used when `RUNTIME` was not
/// explicitly initialized (e.g. in tests).
#[cfg(feature = "rayon")]
fn fallback_rayon_pool(name: &'static str) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .thread_name(move |i| format!("reth-{name}-{i}"))
        .build()
        .expect("failed to build fallback rayon pool")
}

/// Returns a fallback [`BlockingTaskPool`]. Used when `RUNTIME` was not explicitly initialized.
#[cfg(feature = "rayon")]
fn fallback_rpc_pool() -> BlockingTaskPool {
    BlockingTaskPool::new(fallback_rayon_pool("rpc"))
}

/// Returns a fallback [`BlockingTaskGuard`]. Used when `RUNTIME` was not explicitly initialized.
#[cfg(feature = "rayon")]
fn fallback_blocking_guard() -> BlockingTaskGuard {
    BlockingTaskGuard::new(DEFAULT_MAX_BLOCKING_TASKS)
}

#[cold]
#[inline(never)]
fn uninitialized_panic() -> ! {
    panic!("RUNTIME not initialized. Call reth_tasks::RUNTIME.init() before accessing pools.")
}

/// Global runtime singleton that manages async and parallel execution resources.
///
/// Provides centralized access to:
/// - Tokio runtime/handle
/// - [`TaskExecutor`] for spawning async tasks with panic monitoring
/// - General-purpose rayon CPU pool (replaces global rayon pool)
/// - Dedicated RPC blocking pool (for trace calls, `eth_getProof`, etc.)
/// - Dedicated trie proof pool (for parallel state root / multiproof workers)
/// - [`BlockingTaskGuard`] for rate-limiting expensive RPC operations
///
/// All fields use [`OnceLock`] for thread-safe lazy initialization.
/// Getter methods panic with a descriptive message if called before [`GlobalRuntime::init`].
pub struct GlobalRuntime {
    /// The stored configuration.
    config: OnceLock<RuntimeConfig>,
    /// The owned tokio runtime (if we built it).
    runtime: OnceLock<Runtime>,
    /// The tokio handle (always set after init).
    handle: OnceLock<Handle>,
    /// The task executor.
    executor: OnceLock<TaskExecutor>,
    /// General-purpose rayon CPU pool (hashing, signature recovery, `par_iter`, etc.).
    #[cfg(feature = "rayon")]
    cpu_pool: OnceLock<rayon::ThreadPool>,
    /// RPC blocking pool (trace calls, `eth_getProof`, etc.).
    #[cfg(feature = "rayon")]
    rpc_pool: OnceLock<BlockingTaskPool>,
    /// Trie proof computation pool (parallel state root, multiproof workers).
    #[cfg(feature = "rayon")]
    trie_pool: OnceLock<rayon::ThreadPool>,
    /// Storage I/O pool (static file, `RocksDB` writes in `save_blocks`).
    #[cfg(feature = "rayon")]
    storage_pool: OnceLock<rayon::ThreadPool>,
    /// Rate limiter for expensive RPC operations.
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
            rpc_pool: OnceLock::new(),
            #[cfg(feature = "rayon")]
            trie_pool: OnceLock::new(),
            #[cfg(feature = "rayon")]
            storage_pool: OnceLock::new(),
            #[cfg(feature = "rayon")]
            blocking_guard: OnceLock::new(),
        }
    }

    /// Initialize the global runtime with the given configuration.
    ///
    /// Must be called once before using any getter methods. Returns the [`TaskManager`] which
    /// the caller is responsible for driving (polling) to detect critical task panics.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use reth_tasks::{RuntimeConfig, RUNTIME};
    ///
    /// let task_manager = RUNTIME.init(RuntimeConfig::default())?;
    /// ```
    pub fn init(&self, config: RuntimeConfig) -> Result<TaskManager, RuntimeInitError> {
        if self.config.set(config.clone()).is_err() {
            return Err(RuntimeInitError::AlreadyInitialized);
        }

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
                let _ = self.runtime.set(runtime);
                h
            }
            TokioConfig::ExistingHandle(h) => h.clone(),
        };

        let _ = self.handle.set(handle.clone());

        let task_manager = TaskManager::new(handle);
        let executor = task_manager.executor();
        let _ = self.executor.set(executor);

        #[cfg(feature = "rayon")]
        {
            let default_threads = config.rayon.default_thread_count();
            let rpc_threads = config.rayon.rpc_threads.unwrap_or(default_threads);
            let trie_threads = config.rayon.trie_threads.unwrap_or(default_threads);

            // General-purpose CPU pool (replaces rayon global pool for explicit spawns).
            let cpu_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(default_threads)
                .thread_name(|i| format!("reth-cpu-{i}"))
                .build()?;
            let _ = self.cpu_pool.set(cpu_pool);

            // RPC blocking pool for trace calls, eth_getProof, etc.
            let rpc_raw = rayon::ThreadPoolBuilder::new()
                .num_threads(rpc_threads)
                .thread_name(|i| format!("reth-rpc-{i}"))
                .build()?;
            let _ = self.rpc_pool.set(BlockingTaskPool::new(rpc_raw));

            // Trie proof pool for parallel state root and multiproof workers.
            let trie_raw = rayon::ThreadPoolBuilder::new()
                .num_threads(trie_threads)
                .thread_name(|i| format!("reth-trie-{i}"))
                .build()?;
            let _ = self.trie_pool.set(trie_raw);

            let storage_threads =
                config.rayon.storage_threads.unwrap_or(DEFAULT_STORAGE_POOL_THREADS);
            let storage_raw = rayon::ThreadPoolBuilder::new()
                .num_threads(storage_threads)
                .thread_name(|i| format!("reth-storage-{i}"))
                .build()?;
            let _ = self.storage_pool.set(storage_raw);

            let _ =
                self.blocking_guard.set(BlockingTaskGuard::new(config.rayon.max_blocking_tasks));

            debug!(
                default_threads,
                rpc_threads,
                trie_threads,
                storage_threads,
                max_blocking_tasks = config.rayon.max_blocking_tasks,
                "Initialized rayon thread pools"
            );
        }

        debug!("RUNTIME initialized");
        Ok(task_manager)
    }

    /// Initialize from an existing tokio handle with default rayon config.
    pub fn init_with_handle(&self, handle: Handle) -> Result<TaskManager, RuntimeInitError> {
        self.init(RuntimeConfig::with_existing_handle(handle))
    }

    /// Returns `true` if the runtime has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.config.get().is_some()
    }

    /// Get the tokio runtime handle.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized and no tokio runtime is available in the
    /// current context.
    pub fn handle(&self) -> Handle {
        self.handle.get().cloned().unwrap_or_else(fallback_handle)
    }

    /// Get a clone of the [`TaskExecutor`].
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized and no tokio runtime is available in the
    /// current context.
    pub fn executor(&self) -> TaskExecutor {
        self.executor.get().cloned().unwrap_or_else(fallback_executor)
    }

    /// Get the general-purpose rayon CPU thread pool.
    ///
    /// Suitable for CPU-bound parallel work: hashing, signature recovery, `par_iter`, etc.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized (in non-test builds).
    #[cfg(feature = "rayon")]
    pub fn cpu_pool(&self) -> &rayon::ThreadPool {
        self.cpu_pool.get_or_init(|| fallback_rayon_pool("cpu"))
    }

    /// Get the RPC blocking task pool.
    ///
    /// Dedicated pool for expensive RPC operations like `debug_traceTransaction`,
    /// `eth_getProof`, etc. Wraps a rayon pool with async-friendly oneshot channels.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized (in non-test builds).
    #[cfg(feature = "rayon")]
    pub fn rpc_pool(&self) -> &BlockingTaskPool {
        self.rpc_pool.get_or_init(fallback_rpc_pool)
    }

    /// Get the trie proof computation pool.
    ///
    /// Dedicated pool for parallel state root computation, multiproof workers, and
    /// storage proof generation.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized (in non-test builds).
    #[cfg(feature = "rayon")]
    pub fn trie_pool(&self) -> &rayon::ThreadPool {
        self.trie_pool.get_or_init(|| fallback_rayon_pool("trie"))
    }

    /// Get the storage I/O pool.
    ///
    /// Dedicated pool for parallel writes to static files, `RocksDB`, and other storage
    /// backends during block persistence (`save_blocks`).
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized (in non-test builds).
    #[cfg(feature = "rayon")]
    pub fn storage_pool(&self) -> &rayon::ThreadPool {
        self.storage_pool.get_or_init(|| fallback_rayon_pool("storage"))
    }

    /// Get a clone of the [`BlockingTaskGuard`].
    ///
    /// Rate-limits expensive CPU operations like trace execution or proof generation.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized (in non-test builds).
    #[cfg(feature = "rayon")]
    pub fn blocking_guard(&self) -> BlockingTaskGuard {
        self.blocking_guard.get_or_init(fallback_blocking_guard).clone()
    }

    /// Run a closure on the CPU pool, blocking the current thread until completion.
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

    /// Spawn a CPU-bound task on the RPC pool and return an async handle.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized.
    #[cfg(feature = "rayon")]
    pub fn spawn_rpc<F, R>(&self, f: F) -> crate::pool::BlockingTaskHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.rpc_pool().spawn(f)
    }

    /// Run a closure on the trie pool, blocking the current thread until completion.
    ///
    /// # Panics
    ///
    /// Panics if the runtime has not been initialized.
    #[cfg(feature = "rayon")]
    pub fn install_trie<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R + Send,
        R: Send,
    {
        self.trie_pool().install(f)
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
        let count = config.default_thread_count();
        assert!(count >= 1);
    }
}
