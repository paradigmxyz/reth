//! Reth task management.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::{
    metrics::{IncCounterOnDrop, TaskExecutorMetrics},
    shutdown::{signal, GracefulShutdown, GracefulShutdownGuard, Shutdown, Signal},
};
use dyn_clone::DynClone;
use futures_util::{
    future::{select, BoxFuture},
    pin_mut, Future, FutureExt, TryFutureExt,
};
use std::{
    any::Any,
    fmt::{Display, Formatter},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use tracing::{debug, error};
use tracing_futures::Instrument;

pub mod metrics;
pub mod shutdown;

/// A type that can spawn tasks.
///
/// The main purpose of this type is to abstract over [TaskExecutor] so it's more convenient to
/// provide default impls for testing.
///
///
/// # Examples
///
/// Use the [TokioTaskExecutor] that spawns with [tokio::task::spawn]
///
/// ```
/// # async fn t() {
/// use reth_tasks::{TaskSpawner, TokioTaskExecutor};
/// let executor = TokioTaskExecutor::default();
///
/// let task = executor.spawn(Box::pin(async {
///     // -- snip --
/// }));
/// task.await.unwrap();
/// # }
/// ```
///
/// Use the [TaskExecutor] that spawns task directly onto the tokio runtime via the [Handle].
///
/// ```
/// # use reth_tasks::TaskManager;
/// fn t() {
///  use reth_tasks::TaskSpawner;
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let manager = TaskManager::new(rt.handle().clone());
/// let executor = manager.executor();
/// let task = TaskSpawner::spawn(&executor, Box::pin(async {
///     // -- snip --
/// }));
/// rt.block_on(task).unwrap();
/// # }
/// ```
///
/// The [TaskSpawner] trait is [DynClone] so `Box<dyn TaskSpawner>` are also `Clone`.
pub trait TaskSpawner: Send + Sync + Unpin + std::fmt::Debug + DynClone {
    /// Spawns the task onto the runtime.
    /// See also [`Handle::spawn`].
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()>;

    /// This spawns a critical task onto the runtime.
    fn spawn_critical(&self, name: &'static str, fut: BoxFuture<'static, ()>) -> JoinHandle<()>;

    /// Spawns a blocking task onto the runtime.
    fn spawn_blocking(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()>;

    /// This spawns a critical blocking task onto the runtime.
    fn spawn_critical_blocking(
        &self,
        name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()>;
}

dyn_clone::clone_trait_object!(TaskSpawner);

/// An [TaskSpawner] that uses [tokio::task::spawn] to execute tasks
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TokioTaskExecutor;

impl TaskSpawner for TokioTaskExecutor {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::task::spawn(fut)
    }

    fn spawn_critical(&self, _name: &'static str, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::task::spawn(fut)
    }

    fn spawn_blocking(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || tokio::runtime::Handle::current().block_on(fut))
    }

    fn spawn_critical_blocking(
        &self,
        _name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || tokio::runtime::Handle::current().block_on(fut))
    }
}

/// Many reth components require to spawn tasks for long-running jobs. For example `discovery`
/// spawns tasks to handle egress and ingress of udp traffic or `network` that spawns session tasks
/// that handle the traffic to and from a peer.
///
/// To unify how tasks are created, the [`TaskManager`] provides access to the configured Tokio
/// runtime. A [`TaskManager`] stores the [`tokio::runtime::Handle`] it is associated with. In this
/// way it is possible to configure on which runtime a task is executed.
///
/// The main purpose of this type is to be able to monitor if a critical task panicked, for
/// diagnostic purposes, since tokio task essentially fail silently. Therefore, this type is a
/// Stream that yields the name of panicked task, See [`TaskExecutor::spawn_critical`]. In order to
/// execute Tasks use the [`TaskExecutor`] type [`TaskManager::executor`].
#[derive(Debug)]
#[must_use = "TaskManager must be polled to monitor critical tasks"]
pub struct TaskManager {
    /// Handle to the tokio runtime this task manager is associated with.
    ///
    /// See [`Handle`] docs.
    handle: Handle,
    /// Sender half for sending panic signals to this type
    panicked_tasks_tx: UnboundedSender<PanickedTaskError>,
    /// Listens for panicked tasks
    panicked_tasks_rx: UnboundedReceiver<PanickedTaskError>,
    /// The [Signal] to fire when all tasks should be shutdown.
    ///
    /// This is fired when dropped.
    signal: Option<Signal>,
    /// Receiver of the shutdown signal.
    on_shutdown: Shutdown,
    /// How many [GracefulShutdown] tasks are currently active
    graceful_tasks: Arc<AtomicUsize>,
}

// === impl TaskManager ===

impl TaskManager {
    /// Returns a a [TaskManager] over the currently running Runtime.
    ///
    /// # Panics
    ///
    /// This will panic if called outside the context of a Tokio runtime.
    pub fn current() -> Self {
        let handle = Handle::current();
        Self::new(handle)
    }

    /// Create a new instance connected to the given handle's tokio runtime.
    pub fn new(handle: Handle) -> Self {
        let (panicked_tasks_tx, panicked_tasks_rx) = unbounded_channel();
        let (signal, on_shutdown) = signal();
        Self {
            handle,
            panicked_tasks_tx,
            panicked_tasks_rx,
            signal: Some(signal),
            on_shutdown,
            graceful_tasks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a new [`TaskExecutor`] that can spawn new tasks onto the tokio runtime this type is
    /// connected to.
    pub fn executor(&self) -> TaskExecutor {
        TaskExecutor {
            handle: self.handle.clone(),
            on_shutdown: self.on_shutdown.clone(),
            panicked_tasks_tx: self.panicked_tasks_tx.clone(),
            metrics: Default::default(),
            graceful_tasks: Arc::clone(&self.graceful_tasks),
        }
    }

    /// Fires the shutdown signal and awaits until all tasks are shutdown.
    pub fn graceful_shutdown(self) {
        let _ = self.do_graceful_shutdown(None);
    }

    /// Fires the shutdown signal and awaits until all tasks are shutdown.
    ///
    /// Returns true if all tasks were shutdown before the timeout elapsed.
    pub fn graceful_shutdown_with_timeout(self, timeout: std::time::Duration) -> bool {
        self.do_graceful_shutdown(Some(timeout))
    }

    fn do_graceful_shutdown(self, timeout: Option<std::time::Duration>) -> bool {
        drop(self.signal);
        let when = timeout.map(|t| std::time::Instant::now() + t);
        while self.graceful_tasks.load(Ordering::Relaxed) > 0 {
            if when.map(|when| std::time::Instant::now() > when).unwrap_or(false) {
                debug!("graceful shutdown timed out");
                return false
            }
            std::hint::spin_loop();
        }

        debug!("gracefully shut down");
        true
    }
}

/// An endless future that resolves if a critical task panicked.
///
/// See [`TaskExecutor::spawn_critical`]
impl Future for TaskManager {
    type Output = PanickedTaskError;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let err = ready!(self.get_mut().panicked_tasks_rx.poll_recv(cx));
        Poll::Ready(err.expect("stream can not end"))
    }
}

/// Error with the name of the task that panicked and an error downcasted to string, if possible.
#[derive(Debug, thiserror::Error)]
pub struct PanickedTaskError {
    task_name: &'static str,
    error: Option<String>,
}

impl Display for PanickedTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let task_name = self.task_name;
        if let Some(error) = &self.error {
            write!(f, "Critical task `{task_name}` panicked: `{error}`")
        } else {
            write!(f, "Critical task `{task_name}` panicked")
        }
    }
}

impl PanickedTaskError {
    fn new(task_name: &'static str, error: Box<dyn Any>) -> Self {
        let error = match error.downcast::<String>() {
            Ok(value) => Some(*value),
            Err(error) => match error.downcast::<&str>() {
                Ok(value) => Some(value.to_string()),
                Err(_) => None,
            },
        };

        Self { task_name, error }
    }
}

/// A type that can spawn new tokio tasks
#[derive(Debug, Clone)]
pub struct TaskExecutor {
    /// Handle to the tokio runtime this task manager is associated with.
    ///
    /// See [`Handle`] docs.
    handle: Handle,
    /// Receiver of the shutdown signal.
    on_shutdown: Shutdown,
    /// Sender half for sending panic signals to this type
    panicked_tasks_tx: UnboundedSender<PanickedTaskError>,
    // Task Executor Metrics
    metrics: TaskExecutorMetrics,
    /// How many [GracefulShutdown] tasks are currently active
    graceful_tasks: Arc<AtomicUsize>,
}

// === impl TaskExecutor ===

impl TaskExecutor {
    /// Returns the [Handle] to the tokio runtime.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Returns the receiver of the shutdown signal.
    pub fn on_shutdown_signal(&self) -> &Shutdown {
        &self.on_shutdown
    }

    /// Runs a future to completion on this Handle's associated Runtime.
    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.handle.block_on(future)
    }

    /// Spawns a future on the tokio runtime depending on the [TaskKind]
    fn spawn_on_rt<F>(&self, fut: F, task_kind: TaskKind) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match task_kind {
            TaskKind::Default => self.handle.spawn(fut),
            TaskKind::Blocking => {
                let handle = self.handle.clone();
                self.handle.spawn_blocking(move || handle.block_on(fut))
            }
        }
    }

    /// Spawns a regular task depending on the given [TaskKind]
    fn spawn_task_as<F>(&self, fut: F, task_kind: TaskKind) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let on_shutdown = self.on_shutdown.clone();

        // Clone only the specific counter that we need.
        let finished_regular_tasks_metrics = self.metrics.finished_regular_tasks.clone();
        // Wrap the original future to increment the finished tasks counter upon completion
        let task = {
            async move {
                // Create an instance of IncCounterOnDrop with the counter to increment
                let _inc_counter_on_drop = IncCounterOnDrop::new(finished_regular_tasks_metrics);
                pin_mut!(fut);
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
    pub fn spawn<F>(&self, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task_as(fut, TaskKind::Default)
    }

    /// Spawns a blocking task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// See also [`Handle::spawn_blocking`].
    pub fn spawn_blocking<F>(&self, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task_as(fut, TaskKind::Blocking)
    }

    /// Spawns the task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// See also [`Handle::spawn`].
    pub fn spawn_with_signal<F>(&self, f: impl FnOnce(Shutdown) -> F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let on_shutdown = self.on_shutdown.clone();
        let fut = f(on_shutdown);

        let task = fut.in_current_span();

        self.handle.spawn(task)
    }

    /// Spawns a critical task depending on the given [TaskKind]
    fn spawn_critical_as<F>(
        &self,
        name: &'static str,
        fut: F,
        task_kind: TaskKind,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panicked_tasks_tx = self.panicked_tasks_tx.clone();
        let on_shutdown = self.on_shutdown.clone();

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(task_error);
            })
            .in_current_span();

        // Clone only the specific counter that we need.
        let finished_critical_tasks_metrics = self.metrics.finished_critical_tasks.clone();
        let task = async move {
            // Create an instance of IncCounterOnDrop with the counter to increment
            let _inc_counter_on_drop = IncCounterOnDrop::new(finished_critical_tasks_metrics);
            pin_mut!(task);
            let _ = select(on_shutdown, task).await;
        };

        self.spawn_on_rt(task, task_kind)
    }

    /// This spawns a critical blocking task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical_blocking<F>(&self, name: &'static str, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_critical_as(name, fut, TaskKind::Blocking)
    }

    /// This spawns a critical task onto the runtime.
    /// The given future resolves as soon as the [Shutdown] signal is received.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical<F>(&self, name: &'static str, fut: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_critical_as(name, fut, TaskKind::Default)
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
        let panicked_tasks_tx = self.panicked_tasks_tx.clone();
        let on_shutdown = self.on_shutdown.clone();
        let fut = f(on_shutdown);

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(task_error);
            })
            .map(|_| ())
            .in_current_span();

        self.handle.spawn(task)
    }

    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [TaskManager] is notified.
    /// The [TaskManager] will wait until the given future has completed before shutting down.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn t(executor: reth_tasks::TaskExecutor) {
    ///
    /// executor.spawn_critical_with_graceful_shutdown_signal("grace", |shutdown| async move {
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
        let panicked_tasks_tx = self.panicked_tasks_tx.clone();
        let on_shutdown = GracefulShutdown::new(
            self.on_shutdown.clone(),
            GracefulShutdownGuard::new(Arc::clone(&self.graceful_tasks)),
        );
        let fut = f(on_shutdown);

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let task_error = PanickedTaskError::new(name, error);
                error!("{task_error}");
                let _ = panicked_tasks_tx.send(task_error);
            })
            .map(|_| ())
            .in_current_span();

        self.handle.spawn(task)
    }

    /// This spawns a regular task onto the runtime.
    ///
    /// The [TaskManager] will wait until the given future has completed before shutting down.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn t(executor: reth_tasks::TaskExecutor) {
    ///
    /// executor.spawn_with_graceful_shutdown_signal(|shutdown| async move {
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
            self.on_shutdown.clone(),
            GracefulShutdownGuard::new(Arc::clone(&self.graceful_tasks)),
        );
        let fut = f(on_shutdown);

        self.handle.spawn(fut)
    }
}

impl TaskSpawner for TaskExecutor {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        self.metrics.inc_regular_tasks();
        self.spawn(fut)
    }

    fn spawn_critical(&self, name: &'static str, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        self.metrics.inc_critical_tasks();
        TaskExecutor::spawn_critical(self, name, fut)
    }

    fn spawn_blocking(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        self.spawn_blocking(fut)
    }

    fn spawn_critical_blocking(
        &self,
        name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()> {
        TaskExecutor::spawn_critical_blocking(self, name, fut)
    }
}

/// TaskSpawner with extended behaviour
pub trait TaskSpawnerExt: Send + Sync + Unpin + std::fmt::Debug + DynClone {
    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [TaskManager] is notified.
    /// The [TaskManager] will wait until the given future has completed before shutting down.
    fn spawn_critical_with_graceful_shutdown_signal<F>(
        &self,
        name: &'static str,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static;

    /// This spawns a regular task onto the runtime.
    ///
    /// The [TaskManager] will wait until the given future has completed before shutting down.
    fn spawn_with_graceful_shutdown_signal<F>(
        &self,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static;
}

impl TaskSpawnerExt for TaskExecutor {
    fn spawn_critical_with_graceful_shutdown_signal<F>(
        &self,
        name: &'static str,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        TaskExecutor::spawn_critical_with_graceful_shutdown_signal(self, name, f)
    }

    fn spawn_with_graceful_shutdown_signal<F>(
        &self,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        TaskExecutor::spawn_with_graceful_shutdown_signal(self, f)
    }
}

/// Determines how a task is spawned
enum TaskKind {
    /// Spawn the task to the default executor [Handle::spawn]
    Default,
    /// Spawn the task to the blocking executor [Handle::spawn_blocking]
    Blocking,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicBool, time::Duration};

    #[test]
    fn test_cloneable() {
        #[derive(Clone)]
        struct ExecutorWrapper {
            _e: Box<dyn TaskSpawner>,
        }

        let executor: Box<dyn TaskSpawner> = Box::<TokioTaskExecutor>::default();
        let _e = dyn_clone::clone_box(&*executor);

        let e = ExecutorWrapper { _e };
        let _e2 = e;
    }

    #[test]
    fn test_critical() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle);
        let executor = manager.executor();

        executor.spawn_critical("this is a critical task", async { panic!("intentionally panic") });

        runtime.block_on(async move {
            let err = manager.await;
            assert_eq!(err.task_name, "this is a critical task");
            assert_eq!(err.error, Some("intentionally panic".to_string()));
        })
    }

    // Tests that spawned tasks are terminated if the `TaskManager` drops
    #[test]
    fn test_manager_shutdown_critical() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle.clone());
        let executor = manager.executor();

        let (signal, shutdown) = signal();

        executor.spawn_critical("this is a critical task", async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        });

        drop(manager);

        handle.block_on(shutdown);
    }

    // Tests that spawned tasks are terminated if the `TaskManager` drops
    #[test]
    fn test_manager_shutdown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle.clone());
        let executor = manager.executor();

        let (signal, shutdown) = signal();

        executor.spawn(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        }));

        drop(manager);

        handle.block_on(shutdown);
    }

    #[test]
    fn test_manager_graceful_shutdown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle.clone());
        let executor = manager.executor();

        let val = Arc::new(AtomicBool::new(false));
        let c = val.clone();
        executor.spawn_critical_with_graceful_shutdown_signal("grace", |shutdown| async move {
            let _guard = shutdown.await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            c.store(true, Ordering::Relaxed);
        });

        manager.graceful_shutdown();
        assert!(val.load(Ordering::Relaxed));
    }

    #[test]
    fn test_manager_graceful_shutdown_many() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle.clone());
        let executor = manager.executor();
        let _e = executor.clone();

        let counter = Arc::new(AtomicUsize::new(0));
        let num = 10;
        for _ in 0..num {
            let c = counter.clone();
            executor.spawn_critical_with_graceful_shutdown_signal(
                "grace",
                move |shutdown| async move {
                    let _guard = shutdown.await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    c.fetch_add(1, Ordering::SeqCst);
                },
            );
        }

        manager.graceful_shutdown();
        assert_eq!(counter.load(Ordering::Relaxed), num);
    }

    #[test]
    fn test_manager_graceful_shutdown_timeout() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let manager = TaskManager::new(handle.clone());
        let executor = manager.executor();

        let timeout = Duration::from_millis(500);
        let val = Arc::new(AtomicBool::new(false));
        let val2 = val.clone();
        executor.spawn_critical_with_graceful_shutdown_signal("grace", |shutdown| async move {
            let _guard = shutdown.await;
            tokio::time::sleep(timeout * 3).await;
            val2.store(true, Ordering::Relaxed);
            unreachable!("should not be reached");
        });

        manager.graceful_shutdown_with_timeout(timeout);
        assert!(!val.load(Ordering::Relaxed));
    }
}
