//! Centralized management of async and parallel execution.
//!
//! This module provides [`Runtime`], a cheaply cloneable handle that manages:
//! - Tokio runtime (either owned or attached)
//! - Task spawning with shutdown awareness and panic monitoring
//! - Dedicated rayon thread pools for different workloads (with `rayon` feature)
//! - [`BlockingTaskGuard`] for rate-limiting expensive operations (with `rayon` feature)

#[cfg(feature = "rayon")]
use crate::pool::{BlockingTaskGuard, BlockingTaskPool, WorkerPool};
use crate::{
    metrics::{IncCounterOnDrop, TaskExecutorMetrics},
    shutdown::{GracefulShutdown, GracefulShutdownGuard, Shutdown},
    worker_map::WorkerMap,
    PanickedTaskError, TaskEvent, TaskManager,
};
use futures_util::{future::select, Future, FutureExt, TryFutureExt};
#[cfg(feature = "rayon")]
use std::{num::NonZeroUsize, thread::available_parallelism};
use std::{
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::mpsc::UnboundedSender, task::JoinHandle};
use tracing::{debug, error};
use tracing_futures::Instrument;

use tokio::runtime::Runtime as TokioRuntime;

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
    /// Number of threads for the storage I/O pool (static file, `RocksDB` writes in
    /// `save_blocks`). If `None`, uses [`DEFAULT_STORAGE_POOL_THREADS`].
    pub storage_threads: Option<usize>,
    /// Maximum number of concurrent blocking tasks for the RPC guard semaphore.
    pub max_blocking_tasks: usize,
    /// Number of threads for the proof storage worker pool (trie storage proof workers).
    /// If `None`, derived from available parallelism.
    pub proof_storage_worker_threads: Option<usize>,
    /// Number of threads for the proof account worker pool (trie account proof workers).
    /// If `None`, derived from available parallelism.
    pub proof_account_worker_threads: Option<usize>,
    /// Number of threads for the prewarming pool (execution prewarming workers).
    /// If `None`, derived from available parallelism.
    pub prewarming_threads: Option<usize>,
}

#[cfg(feature = "rayon")]
impl Default for RayonConfig {
    fn default() -> Self {
        Self {
            cpu_threads: None,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            rpc_threads: None,
            storage_threads: None,
            max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
            proof_storage_worker_threads: None,
            proof_account_worker_threads: None,
            prewarming_threads: None,
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

    /// Set the number of threads for the storage I/O pool.
    pub const fn with_storage_threads(mut self, storage_threads: usize) -> Self {
        self.storage_threads = Some(storage_threads);
        self
    }

    /// Set the number of threads for the proof storage worker pool.
    pub const fn with_proof_storage_worker_threads(
        mut self,
        proof_storage_worker_threads: usize,
    ) -> Self {
        self.proof_storage_worker_threads = Some(proof_storage_worker_threads);
        self
    }

    /// Set the number of threads for the proof account worker pool.
    pub const fn with_proof_account_worker_threads(
        mut self,
        proof_account_worker_threads: usize,
    ) -> Self {
        self.proof_account_worker_threads = Some(proof_account_worker_threads);
        self
    }

    /// Set the number of threads for the prewarming pool.
    pub const fn with_prewarming_threads(mut self, prewarming_threads: usize) -> Self {
        self.prewarming_threads = Some(prewarming_threads);
        self
    }

    /// Compute the default number of threads based on available parallelism.
    fn default_thread_count(&self) -> usize {
        // TODO: reserved_cpu_cores is currently ignored because subtracting from thread pool
        // sizes doesn't actually reserve CPU cores for other processes.
        let _ = self.reserved_cpu_cores;
        self.cpu_threads.unwrap_or_else(|| available_parallelism().map_or(1, NonZeroUsize::get))
    }
}

/// Configuration for building a [`Runtime`].
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    /// Tokio runtime configuration.
    pub tokio: TokioConfig,
    /// Rayon thread pool configuration.
    #[cfg(feature = "rayon")]
    pub rayon: RayonConfig,
}

impl RuntimeConfig {
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

/// Error returned when [`RuntimeBuilder::build`] fails.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBuildError {
    /// Failed to build the tokio runtime.
    #[error("Failed to build tokio runtime: {0}")]
    TokioBuild(#[from] std::io::Error),
    /// Failed to build a rayon thread pool.
    #[cfg(feature = "rayon")]
    #[error("Failed to build rayon thread pool: {0}")]
    RayonBuild(#[from] rayon::ThreadPoolBuildError),
}

// ── RuntimeInner ──────────────────────────────────────────────────────

struct RuntimeInner {
    /// Owned tokio runtime, if we built one. Kept alive via the `Arc<RuntimeInner>`.
    _tokio_runtime: Option<TokioRuntime>,
    /// Handle to the tokio runtime.
    handle: Handle,
    /// Receiver of the shutdown signal.
    on_shutdown: Shutdown,
    /// Sender half for sending task events to the [`TaskManager`].
    task_events_tx: UnboundedSender<TaskEvent>,
    /// Task executor metrics.
    metrics: TaskExecutorMetrics,
    /// How many [`GracefulShutdown`] tasks are currently active.
    graceful_tasks: Arc<AtomicUsize>,
    /// General-purpose rayon CPU pool.
    #[cfg(feature = "rayon")]
    cpu_pool: rayon::ThreadPool,
    /// RPC blocking pool.
    #[cfg(feature = "rayon")]
    rpc_pool: BlockingTaskPool,
    /// Storage I/O pool.
    #[cfg(feature = "rayon")]
    storage_pool: rayon::ThreadPool,
    /// Rate limiter for expensive RPC operations.
    #[cfg(feature = "rayon")]
    blocking_guard: BlockingTaskGuard,
    /// Proof storage worker pool (trie storage proof computation).
    #[cfg(feature = "rayon")]
    proof_storage_worker_pool: WorkerPool,
    /// Proof account worker pool (trie account proof computation).
    #[cfg(feature = "rayon")]
    proof_account_worker_pool: WorkerPool,
    /// Prewarming pool (execution prewarming workers).
    #[cfg(feature = "rayon")]
    prewarming_pool: WorkerPool,
    /// Named single-thread worker map. Each unique name gets a dedicated OS thread
    /// that is reused across all tasks submitted under that name.
    worker_map: WorkerMap,
    /// Handle to the spawned [`TaskManager`] background task.
    /// The task monitors critical tasks for panics and fires the shutdown signal.
    /// Can be taken via [`Runtime::take_task_manager_handle`] to poll for panic errors.
    task_manager_handle: Mutex<Option<JoinHandle<Result<(), PanickedTaskError>>>>,
}

// ── Runtime ───────────────────────────────────────────────────────────

/// A cheaply cloneable handle to the runtime resources.
///
/// Wraps an `Arc<RuntimeInner>` and provides access to:
/// - The tokio [`Handle`]
/// - Task spawning with shutdown awareness and panic monitoring
/// - Rayon thread pools (with `rayon` feature)
#[derive(Clone)]
pub struct Runtime(Arc<RuntimeInner>);

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").field("handle", &self.0.handle).finish()
    }
}

// ── Pool accessors ────────────────────────────────────────────────────

impl Runtime {
    /// Takes the [`TaskManager`] handle out of this runtime, if one is stored.
    ///
    /// The handle resolves with `Err(PanickedTaskError)` if a critical task panicked,
    /// or `Ok(())` if shutdown was requested. If not taken, the background task still
    /// runs and logs panics at `debug!` level.
    pub fn take_task_manager_handle(&self) -> Option<JoinHandle<Result<(), PanickedTaskError>>> {
        self.0.task_manager_handle.lock().unwrap().take()
    }

    /// Returns the tokio runtime [`Handle`].
    pub fn handle(&self) -> &Handle {
        &self.0.handle
    }

    /// Get the general-purpose rayon CPU thread pool.
    #[cfg(feature = "rayon")]
    pub fn cpu_pool(&self) -> &rayon::ThreadPool {
        &self.0.cpu_pool
    }

    /// Get the RPC blocking task pool.
    #[cfg(feature = "rayon")]
    pub fn rpc_pool(&self) -> &BlockingTaskPool {
        &self.0.rpc_pool
    }

    /// Get the storage I/O pool.
    #[cfg(feature = "rayon")]
    pub fn storage_pool(&self) -> &rayon::ThreadPool {
        &self.0.storage_pool
    }

    /// Get a clone of the [`BlockingTaskGuard`].
    #[cfg(feature = "rayon")]
    pub fn blocking_guard(&self) -> BlockingTaskGuard {
        self.0.blocking_guard.clone()
    }

    /// Get the proof storage worker pool.
    #[cfg(feature = "rayon")]
    pub fn proof_storage_worker_pool(&self) -> &WorkerPool {
        &self.0.proof_storage_worker_pool
    }

    /// Get the proof account worker pool.
    #[cfg(feature = "rayon")]
    pub fn proof_account_worker_pool(&self) -> &WorkerPool {
        &self.0.proof_account_worker_pool
    }

    /// Get the prewarming pool.
    #[cfg(feature = "rayon")]
    pub fn prewarming_pool(&self) -> &WorkerPool {
        &self.0.prewarming_pool
    }
}

// ── Test helpers ──────────────────────────────────────────────────────

impl Runtime {
    /// Creates a lightweight [`Runtime`] for tests with minimal thread pools.
    ///
    /// If called from within a tokio runtime (e.g. `#[tokio::test]`), attaches to the existing
    /// handle to avoid shutdown panics when the test runtime is dropped.
    pub fn test() -> Self {
        let config = match Handle::try_current() {
            Ok(handle) => Self::test_config().with_tokio(TokioConfig::existing_handle(handle)),
            Err(_) => Self::test_config(),
        };
        RuntimeBuilder::new(config).build().expect("failed to build test Runtime")
    }

    const fn test_config() -> RuntimeConfig {
        RuntimeConfig {
            tokio: TokioConfig::Owned {
                worker_threads: Some(2),
                thread_keep_alive: DEFAULT_THREAD_KEEP_ALIVE,
                thread_name: "tokio-test",
            },
            #[cfg(feature = "rayon")]
            rayon: RayonConfig {
                cpu_threads: Some(2),
                reserved_cpu_cores: 0,
                rpc_threads: Some(2),
                storage_threads: Some(2),
                max_blocking_tasks: 16,
                proof_storage_worker_threads: Some(2),
                proof_account_worker_threads: Some(2),
                prewarming_threads: Some(2),
            },
        }
    }
}

// ── Spawn methods ─────────────────────────────────────────────────────

/// Determines how a task is spawned.
enum TaskKind {
    /// Spawn the task to the default executor [`Handle::spawn`].
    Default,
    /// Spawn the task to the blocking executor [`Handle::spawn_blocking`].
    Blocking,
}

impl Runtime {
    /// Returns the receiver of the shutdown signal.
    pub fn on_shutdown_signal(&self) -> &Shutdown {
        &self.0.on_shutdown
    }

    /// Spawns a future on the tokio runtime depending on the [`TaskKind`].
    fn spawn_on_rt<F>(&self, fut: F, task_kind: TaskKind) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match task_kind {
            TaskKind::Default => self.0.handle.spawn(fut),
            TaskKind::Blocking => {
                let handle = self.0.handle.clone();
                self.0.handle.spawn_blocking(move || handle.block_on(fut))
            }
        }
    }

    /// Spawns a regular task depending on the given [`TaskKind`].
    fn spawn_task_as<F>(&self, fut: F, task_kind: TaskKind) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match task_kind {
            TaskKind::Default => self.0.metrics.inc_regular_tasks(),
            TaskKind::Blocking => self.0.metrics.inc_regular_blocking_tasks(),
        }
        let on_shutdown = self.0.on_shutdown.clone();

        let finished_counter = match task_kind {
            TaskKind::Default => self.0.metrics.finished_regular_tasks_total.clone(),
            TaskKind::Blocking => self.0.metrics.finished_regular_blocking_tasks_total.clone(),
        };

        let task = {
            async move {
                let _inc_counter_on_drop = IncCounterOnDrop::new(finished_counter);
                let fut = pin!(fut);
                let _ = select(on_shutdown, fut).await;
            }
        }
        .in_current_span();

        self.spawn_on_rt(task, task_kind)
    }

    /// Spawns the task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// See also [`Handle::spawn`].
    pub fn spawn_task<F>(&self, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task_as(fut, TaskKind::Default)
    }

    /// Spawns a blocking task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// See also [`Handle::spawn_blocking`].
    pub fn spawn_blocking_task<F>(&self, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task_as(fut, TaskKind::Blocking)
    }

    /// Spawns a blocking closure directly on the tokio runtime, bypassing shutdown
    /// awareness. Useful for raw CPU-bound work.
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.0.handle.spawn_blocking(func)
    }

    /// Spawns a blocking closure on a dedicated, named OS thread.
    ///
    /// Unlike [`spawn_blocking`](Self::spawn_blocking) which uses tokio's blocking thread pool,
    /// this reuses the same OS thread for all tasks submitted under the same `name`. The thread
    /// is created lazily on first use and its OS thread name is set to `name`.
    ///
    /// This is useful for tasks that benefit from running on a stable thread, e.g. for
    /// thread-local state reuse or to avoid thread creation overhead on hot paths.
    ///
    /// Returns a [`LazyHandle`](crate::LazyHandle) handle that resolves on first access and caches
    /// the result.
    pub fn spawn_blocking_named<F, R>(&self, name: &'static str, func: F) -> crate::LazyHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        crate::LazyHandle::new(self.0.worker_map.spawn_on(name, func))
    }

    /// Spawns the task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// See also [`Handle::spawn`].
    pub fn spawn_with_signal<F>(&self, f: impl FnOnce(Shutdown) -> F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let on_shutdown = self.0.on_shutdown.clone();
        let fut = f(on_shutdown);
        let task = fut.in_current_span();
        self.0.handle.spawn(task)
    }

    /// Spawns a critical task depending on the given [`TaskKind`].
    fn spawn_critical_as<F>(
        &self,
        name: &'static str,
        fut: F,
        task_kind: TaskKind,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.0.metrics.inc_critical_tasks();
        let panicked_tasks_tx = self.0.task_events_tx.clone();
        let on_shutdown = self.0.on_shutdown.clone();

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(TaskEvent::Panic(task_error));
            })
            .in_current_span();

        let finished_critical_tasks_total_metrics =
            self.0.metrics.finished_critical_tasks_total.clone();
        let task = async move {
            let _inc_counter_on_drop = IncCounterOnDrop::new(finished_critical_tasks_total_metrics);
            let task = pin!(task);
            let _ = select(on_shutdown, task).await;
        };

        self.spawn_on_rt(task, task_kind)
    }

    /// This spawns a critical task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical_task<F>(&self, name: &'static str, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_critical_as(name, fut, TaskKind::Default)
    }

    /// This spawns a critical blocking task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical_blocking_task<F>(&self, name: &'static str, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_critical_as(name, fut, TaskKind::Blocking)
    }

    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical_with_shutdown_signal<F>(
        &self,
        name: &'static str,
        f: impl FnOnce(Shutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panicked_tasks_tx = self.0.task_events_tx.clone();
        let on_shutdown = self.0.on_shutdown.clone();
        let fut = f(on_shutdown);

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(TaskEvent::Panic(task_error));
            })
            .map(drop)
            .in_current_span();

        self.0.handle.spawn(task)
    }

    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    /// The [`TaskManager`] will wait until the given future has completed before shutting down.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn t(executor: reth_tasks::TaskExecutor) {
    ///
    /// executor.spawn_critical_with_graceful_shutdown_signal("grace", async move |shutdown| {
    ///     // await the shutdown signal
    ///     let guard = shutdown.await;
    ///     // do work before exiting the program
    ///     tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    ///     // allow graceful shutdown
    ///     drop(guard);
    /// });
    /// # }
    /// ```
    pub fn spawn_critical_with_graceful_shutdown_signal<F>(
        &self,
        name: &'static str,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panicked_tasks_tx = self.0.task_events_tx.clone();
        let on_shutdown = GracefulShutdown::new(
            self.0.on_shutdown.clone(),
            GracefulShutdownGuard::new(Arc::clone(&self.0.graceful_tasks)),
        );
        let fut = f(on_shutdown);

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(TaskEvent::Panic(task_error));
            })
            .map(drop)
            .in_current_span();

        self.0.handle.spawn(task)
    }

    /// This spawns a regular task onto the runtime.
    ///
    /// The [`TaskManager`] will wait until the given future has completed before shutting down.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn t(executor: reth_tasks::TaskExecutor) {
    ///
    /// executor.spawn_with_graceful_shutdown_signal(async move |shutdown| {
    ///     // await the shutdown signal
    ///     let guard = shutdown.await;
    ///     // do work before exiting the program
    ///     tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    ///     // allow graceful shutdown
    ///     drop(guard);
    /// });
    /// # }
    /// ```
    pub fn spawn_with_graceful_shutdown_signal<F>(
        &self,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let on_shutdown = GracefulShutdown::new(
            self.0.on_shutdown.clone(),
            GracefulShutdownGuard::new(Arc::clone(&self.0.graceful_tasks)),
        );
        let fut = f(on_shutdown);

        self.0.handle.spawn(fut)
    }

    /// Sends a request to the `TaskManager` to initiate a graceful shutdown.
    ///
    /// Caution: This will terminate the entire program.
    pub fn initiate_graceful_shutdown(
        &self,
    ) -> Result<GracefulShutdown, tokio::sync::mpsc::error::SendError<()>> {
        self.0
            .task_events_tx
            .send(TaskEvent::GracefulShutdown)
            .map_err(|_send_error_with_task_event| tokio::sync::mpsc::error::SendError(()))?;

        Ok(GracefulShutdown::new(
            self.0.on_shutdown.clone(),
            GracefulShutdownGuard::new(Arc::clone(&self.0.graceful_tasks)),
        ))
    }

    /// Fires the shutdown signal and waits until all graceful tasks complete.
    pub fn graceful_shutdown(&self) {
        let _ = self.do_graceful_shutdown(None);
    }

    /// Fires the shutdown signal and waits until all graceful tasks complete or the timeout
    /// elapses.
    ///
    /// Returns `true` if all tasks completed before the timeout.
    pub fn graceful_shutdown_with_timeout(&self, timeout: Duration) -> bool {
        self.do_graceful_shutdown(Some(timeout))
    }

    fn do_graceful_shutdown(&self, timeout: Option<Duration>) -> bool {
        let _ = self.0.task_events_tx.send(TaskEvent::GracefulShutdown);
        let deadline = timeout.map(|t| Instant::now() + t);
        while self.0.graceful_tasks.load(Ordering::SeqCst) > 0 {
            if deadline.is_some_and(|d| Instant::now() > d) {
                debug!("graceful shutdown timed out");
                return false;
            }
            std::thread::yield_now();
        }
        debug!("gracefully shut down");
        true
    }
}

// ── RuntimeBuilder ────────────────────────────────────────────────────

/// Builder for constructing a [`Runtime`].
#[derive(Debug, Clone)]
pub struct RuntimeBuilder {
    config: RuntimeConfig,
}

impl RuntimeBuilder {
    /// Create a new builder with the given configuration.
    pub const fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    /// Build the [`Runtime`].
    ///
    /// The [`TaskManager`] is automatically spawned as a background task that monitors
    /// critical tasks for panics. Use [`Runtime::take_task_manager_handle`] to extract
    /// the join handle if you need to poll for panic errors.
    #[tracing::instrument(name = "RuntimeBuilder::build", level = "debug", skip_all)]
    pub fn build(self) -> Result<Runtime, RuntimeBuildError> {
        debug!(?self.config, "Building runtime");
        let config = self.config;

        let (owned_runtime, handle) = match &config.tokio {
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
                (Some(runtime), h)
            }
            TokioConfig::ExistingHandle(h) => (None, h.clone()),
        };

        let (task_manager, on_shutdown, task_events_tx, graceful_tasks) =
            TaskManager::new_parts(handle.clone());

        #[cfg(feature = "rayon")]
        let (
            cpu_pool,
            rpc_pool,
            storage_pool,
            blocking_guard,
            proof_storage_worker_pool,
            proof_account_worker_pool,
            prewarming_pool,
        ) = {
            let default_threads = config.rayon.default_thread_count();
            let rpc_threads = config.rayon.rpc_threads.unwrap_or(default_threads);

            let cpu_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(default_threads)
                .thread_name(|i| format!("cpu-{i:02}"))
                .build()?;

            let rpc_raw = rayon::ThreadPoolBuilder::new()
                .num_threads(rpc_threads)
                .thread_name(|i| format!("rpc-{i:02}"))
                .build()?;
            let rpc_pool = BlockingTaskPool::new(rpc_raw);

            let storage_threads =
                config.rayon.storage_threads.unwrap_or(DEFAULT_STORAGE_POOL_THREADS);
            let storage_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(storage_threads)
                .thread_name(|i| format!("storage-{i:02}"))
                .build()?;

            let blocking_guard = BlockingTaskGuard::new(config.rayon.max_blocking_tasks);

            let proof_storage_worker_threads =
                config.rayon.proof_storage_worker_threads.unwrap_or(default_threads * 2);
            let proof_storage_worker_pool = WorkerPool::from_builder(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(proof_storage_worker_threads)
                    .thread_name(|i| format!("proof-strg-{i:02}")),
            )?;

            let proof_account_worker_threads =
                config.rayon.proof_account_worker_threads.unwrap_or(default_threads * 2);
            let proof_account_worker_pool = WorkerPool::from_builder(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(proof_account_worker_threads)
                    .thread_name(|i| format!("proof-acct-{i:02}")),
            )?;

            let prewarming_threads = config.rayon.prewarming_threads.unwrap_or(default_threads);
            let prewarming_pool = WorkerPool::from_builder(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(prewarming_threads)
                    .thread_name(|i| format!("prewarm-{i:02}")),
            )?;

            debug!(
                default_threads,
                rpc_threads,
                storage_threads,
                proof_storage_worker_threads,
                proof_account_worker_threads,
                prewarming_threads,
                max_blocking_tasks = config.rayon.max_blocking_tasks,
                "Initialized rayon thread pools"
            );

            (
                cpu_pool,
                rpc_pool,
                storage_pool,
                blocking_guard,
                proof_storage_worker_pool,
                proof_account_worker_pool,
                prewarming_pool,
            )
        };

        let task_manager_handle = handle.spawn(async move {
            let result = task_manager.await;
            if let Err(ref err) = result {
                debug!("{err}");
            }
            result
        });

        let inner = RuntimeInner {
            _tokio_runtime: owned_runtime,
            handle,
            on_shutdown,
            task_events_tx,
            metrics: Default::default(),
            graceful_tasks,
            #[cfg(feature = "rayon")]
            cpu_pool,
            #[cfg(feature = "rayon")]
            rpc_pool,
            #[cfg(feature = "rayon")]
            storage_pool,
            #[cfg(feature = "rayon")]
            blocking_guard,
            #[cfg(feature = "rayon")]
            proof_storage_worker_pool,
            #[cfg(feature = "rayon")]
            proof_account_worker_pool,
            #[cfg(feature = "rayon")]
            prewarming_pool,
            worker_map: WorkerMap::new(),
            task_manager_handle: Mutex::new(Some(task_manager_handle)),
        };

        Ok(Runtime(Arc::new(inner)))
    }
}

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
        let rt = TokioRuntime::new().unwrap();
        let config =
            Runtime::test_config().with_tokio(TokioConfig::existing_handle(rt.handle().clone()));
        assert!(matches!(config.tokio, TokioConfig::ExistingHandle(_)));
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn test_rayon_config_thread_count() {
        let config = RayonConfig::default();
        let count = config.default_thread_count();
        assert!(count >= 1);
    }

    #[test]
    fn test_runtime_builder() {
        let rt = TokioRuntime::new().unwrap();
        let config =
            Runtime::test_config().with_tokio(TokioConfig::existing_handle(rt.handle().clone()));
        let runtime = RuntimeBuilder::new(config).build().unwrap();
        let _ = runtime.handle();
    }
}
