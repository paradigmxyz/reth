//! Reth task management.
//!
//! # Feature Flags
//!
//! - `rayon`: Enable rayon thread pool for blocking tasks.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use crate::shutdown::{signal, GracefulShutdown, Shutdown, Signal};
use dyn_clone::DynClone;
use futures_util::future::BoxFuture;
use std::{
    any::Any,
    fmt::{Display, Formatter},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    thread,
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use tracing::debug;

pub mod metrics;
pub mod runtime;
pub mod shutdown;

#[cfg(feature = "rayon")]
pub mod pool;

#[cfg(feature = "rayon")]
pub use runtime::RayonConfig;
pub use runtime::{Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeConfig, TokioConfig};

/// A [`TaskExecutor`] is now an alias for [`Runtime`].
pub type TaskExecutor = Runtime;

/// Spawns an OS thread with the current tokio runtime context propagated.
///
/// This function captures the current tokio runtime handle (if available) and enters it
/// in the newly spawned thread. This ensures that code running in the spawned thread can
/// use [`Handle::current()`], [`Handle::spawn_blocking()`], and other tokio utilities that
/// require a runtime context.
#[track_caller]
pub fn spawn_os_thread<F, T>(name: &str, f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = Handle::try_current().ok();
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _guard = handle.as_ref().map(Handle::enter);
            f()
        })
        .unwrap_or_else(|e| panic!("failed to spawn thread {name:?}: {e}"))
}

/// Spawns a scoped OS thread with the current tokio runtime context propagated.
///
/// This is the scoped thread version of [`spawn_os_thread`], for use with [`std::thread::scope`].
#[track_caller]
pub fn spawn_scoped_os_thread<'scope, 'env, F, T>(
    scope: &'scope thread::Scope<'scope, 'env>,
    name: &str,
    f: F,
) -> thread::ScopedJoinHandle<'scope, T>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    let handle = Handle::try_current().ok();
    thread::Builder::new()
        .name(name.to_string())
        .spawn_scoped(scope, move || {
            let _guard = handle.as_ref().map(Handle::enter);
            f()
        })
        .unwrap_or_else(|e| panic!("failed to spawn scoped thread {name:?}: {e}"))
}

/// A type that can spawn tasks.
///
/// The main purpose of this type is to abstract over [`Runtime`] so it's more convenient to
/// provide default impls for testing.
///
///
/// # Examples
///
/// Use the [`TokioTaskExecutor`] that spawns with [`tokio::task::spawn`]
///
/// ```
/// # async fn t() {
/// use reth_tasks::{TaskSpawner, TokioTaskExecutor};
/// let executor = TokioTaskExecutor::default();
///
/// let task = executor.spawn_task(Box::pin(async {
///     // -- snip --
/// }));
/// task.await.unwrap();
/// # }
/// ```
///
/// Use the [`Runtime`] that spawns task directly onto the tokio runtime via the [Handle].
///
/// ```
/// # use reth_tasks::Runtime;
/// fn t() {
///  use reth_tasks::TaskSpawner;
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let runtime = Runtime::with_existing_handle(rt.handle().clone()).unwrap();
/// let task = TaskSpawner::spawn_task(&runtime, Box::pin(async {
///     // -- snip --
/// }));
/// rt.block_on(task).unwrap();
/// # }
/// ```
///
/// The [`TaskSpawner`] trait is [`DynClone`] so `Box<dyn TaskSpawner>` are also `Clone`.
#[auto_impl::auto_impl(&, Arc)]
pub trait TaskSpawner: Send + Sync + Unpin + std::fmt::Debug + DynClone {
    /// Spawns the task onto the runtime.
    /// See also [`Handle::spawn`].
    fn spawn_task(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()>;

    /// This spawns a critical task onto the runtime.
    fn spawn_critical_task(
        &self,
        name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()>;

    /// Spawns a blocking task onto the runtime.
    fn spawn_blocking_task(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()>;

    /// This spawns a critical blocking task onto the runtime.
    fn spawn_critical_blocking_task(
        &self,
        name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()>;
}

dyn_clone::clone_trait_object!(TaskSpawner);

/// An [`TaskSpawner`] that uses [`tokio::task::spawn`] to execute tasks
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TokioTaskExecutor;

impl TokioTaskExecutor {
    /// Converts the instance to a boxed [`TaskSpawner`].
    pub fn boxed(self) -> Box<dyn TaskSpawner + 'static> {
        Box::new(self)
    }
}

impl TaskSpawner for TokioTaskExecutor {
    fn spawn_task(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::task::spawn(fut)
    }

    fn spawn_critical_task(
        &self,
        _name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()> {
        tokio::task::spawn(fut)
    }

    fn spawn_blocking_task(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || tokio::runtime::Handle::current().block_on(fut))
    }

    fn spawn_critical_blocking_task(
        &self,
        _name: &'static str,
        fut: BoxFuture<'static, ()>,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || tokio::runtime::Handle::current().block_on(fut))
    }
}

/// Monitors critical tasks for panics and manages graceful shutdown.
///
/// The main purpose of this type is to be able to monitor if a critical task panicked, for
/// diagnostic purposes, since tokio tasks essentially fail silently. Therefore, this type is a
/// Future that resolves with the name of the panicked task. See [`Runtime::spawn_critical_task`].
///
/// Automatically spawned as a background task when building a [`Runtime`]. Use
/// [`Runtime::take_task_manager_handle`] to extract the join handle if you need to poll for
/// panic errors directly.
#[derive(Debug)]
#[must_use = "TaskManager must be polled to monitor critical tasks"]
pub struct TaskManager {
    /// Receiver for task events.
    task_events_rx: UnboundedReceiver<TaskEvent>,
    /// The [Signal] to fire when all tasks should be shutdown.
    ///
    /// This is fired when dropped.
    signal: Option<Signal>,
    /// How many [`GracefulShutdown`] tasks are currently active.
    graceful_tasks: Arc<AtomicUsize>,
}

// === impl TaskManager ===

impl TaskManager {
    /// Create a new [`TaskManager`] without an associated [`Runtime`], returning
    /// the shutdown/event primitives for [`RuntimeBuilder`] to wire up.
    pub(crate) fn new_parts(
        _handle: Handle,
    ) -> (Self, Shutdown, UnboundedSender<TaskEvent>, Arc<AtomicUsize>) {
        let (task_events_tx, task_events_rx) = unbounded_channel();
        let (signal, on_shutdown) = signal();
        let graceful_tasks = Arc::new(AtomicUsize::new(0));
        let manager = Self {
            task_events_rx,
            signal: Some(signal),
            graceful_tasks: Arc::clone(&graceful_tasks),
        };
        (manager, on_shutdown, task_events_tx, graceful_tasks)
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
        let deadline = timeout.map(|t| std::time::Instant::now() + t);
        while self.graceful_tasks.load(Ordering::SeqCst) > 0 {
            if deadline.is_some_and(|d| std::time::Instant::now() > d) {
                debug!("graceful shutdown timed out");
                return false;
            }
            thread::yield_now();
        }
        debug!("gracefully shut down");
        true
    }
}

/// An endless future that resolves if a critical task panicked.
///
/// See [`Runtime::spawn_critical_task`]
impl std::future::Future for TaskManager {
    type Output = Result<(), PanickedTaskError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.as_mut().get_mut().task_events_rx.poll_recv(cx)) {
            Some(TaskEvent::Panic(err)) => Poll::Ready(Err(err)),
            Some(TaskEvent::GracefulShutdown) | None => {
                if let Some(signal) = self.get_mut().signal.take() {
                    signal.fire();
                }
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// Error with the name of the task that panicked and an error downcasted to string, if possible.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
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
    pub(crate) fn new(task_name: &'static str, error: Box<dyn Any>) -> Self {
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

/// Represents the events that the `TaskManager`'s main future can receive.
#[derive(Debug)]
pub(crate) enum TaskEvent {
    /// Indicates that a critical task has panicked.
    Panic(PanickedTaskError),
    /// A signal requesting a graceful shutdown of the `TaskManager`.
    GracefulShutdown,
}

/// `TaskSpawner` with extended behaviour
#[auto_impl::auto_impl(&, Arc)]
pub trait TaskSpawnerExt: Send + Sync + Unpin + std::fmt::Debug + DynClone {
    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    /// The [`TaskManager`] will wait until the given future has completed before shutting down.
    fn spawn_critical_with_graceful_shutdown_signal<F>(
        &self,
        name: &'static str,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static;

    /// This spawns a regular task onto the runtime.
    ///
    /// The [`TaskManager`] will wait until the given future has completed before shutting down.
    fn spawn_with_graceful_shutdown_signal<F>(
        &self,
        f: impl FnOnce(GracefulShutdown) -> F,
    ) -> JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

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
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();
        let handle = rt.take_task_manager_handle().unwrap();

        rt.spawn_critical_task("this is a critical task", async { panic!("intentionally panic") });

        runtime.block_on(async move {
            let err_result = handle.await.unwrap();
            assert!(err_result.is_err(), "Expected TaskManager to return an error due to panic");
            let panicked_err = err_result.unwrap_err();

            assert_eq!(panicked_err.task_name, "this is a critical task");
            assert_eq!(panicked_err.error, Some("intentionally panic".to_string()));
        })
    }

    #[test]
    fn test_manager_shutdown_critical() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();

        let (signal, shutdown) = signal();

        rt.spawn_critical_task("this is a critical task", async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        });

        rt.graceful_shutdown();

        runtime.block_on(shutdown);
    }

    #[test]
    fn test_manager_shutdown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();

        let (signal, shutdown) = signal();

        rt.spawn_task(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        }));

        rt.graceful_shutdown();

        runtime.block_on(shutdown);
    }

    #[test]
    fn test_manager_graceful_shutdown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();

        let val = Arc::new(AtomicBool::new(false));
        let c = val.clone();
        rt.spawn_critical_with_graceful_shutdown_signal("grace", |shutdown| async move {
            let _guard = shutdown.await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            c.store(true, Ordering::Relaxed);
        });

        rt.graceful_shutdown();
        assert!(val.load(Ordering::Relaxed));
    }

    #[test]
    fn test_manager_graceful_shutdown_many() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();

        let counter = Arc::new(AtomicUsize::new(0));
        let num = 10;
        for _ in 0..num {
            let c = counter.clone();
            rt.spawn_critical_with_graceful_shutdown_signal("grace", move |shutdown| async move {
                let _guard = shutdown.await;
                tokio::time::sleep(Duration::from_millis(200)).await;
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        rt.graceful_shutdown();
        assert_eq!(counter.load(Ordering::Relaxed), num);
    }

    #[test]
    fn test_manager_graceful_shutdown_timeout() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();

        let timeout = Duration::from_millis(500);
        let val = Arc::new(AtomicBool::new(false));
        let val2 = val.clone();
        rt.spawn_critical_with_graceful_shutdown_signal("grace", |shutdown| async move {
            let _guard = shutdown.await;
            tokio::time::sleep(timeout * 3).await;
            val2.store(true, Ordering::Relaxed);
            unreachable!("should not be reached");
        });

        rt.graceful_shutdown_with_timeout(timeout);
        assert!(!val.load(Ordering::Relaxed));
    }

    #[test]
    fn can_build_runtime() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();
        let _handle = rt.handle();
    }

    #[test]
    fn test_graceful_shutdown_triggered_by_executor() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rt = Runtime::with_existing_handle(runtime.handle().clone()).unwrap();
        let task_manager_handle = rt.take_task_manager_handle().unwrap();

        let task_did_shutdown_flag = Arc::new(AtomicBool::new(false));
        let flag_clone = task_did_shutdown_flag.clone();

        let spawned_task_handle = rt.spawn_with_signal(|shutdown_signal| async move {
            shutdown_signal.await;
            flag_clone.store(true, Ordering::SeqCst);
        });

        let send_result = rt.initiate_graceful_shutdown();
        assert!(send_result.is_ok());

        let manager_final_result = runtime.block_on(task_manager_handle);
        assert!(manager_final_result.is_ok(), "TaskManager task should not panic");
        assert_eq!(manager_final_result.unwrap(), Ok(()));

        let task_join_result = runtime.block_on(spawned_task_handle);
        assert!(task_join_result.is_ok());

        assert!(task_did_shutdown_flag.load(Ordering::Relaxed));
    }
}
