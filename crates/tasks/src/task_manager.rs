//! Native Tokio task management and blocking-task integration.

use crate::{
    shutdown::{signal, Shutdown, Signal},
    thread, time, Runtime,
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
use tracing::debug;

/// A [`TaskExecutor`] is now an alias for [`Runtime`].
pub type TaskExecutor = Runtime;

/// Spawns a thread on the selected blocking backend.
///
/// Natively, this captures the current Tokio handle, if available, and enters it on the new
/// OS thread so the workload can use [`Handle::current()`] and [`Handle::spawn_blocking()`].
/// Under `fuzz`, logical threads share the calling OS thread's Tokio context. If the workload
/// needs Tokio, enter the desired [`Handle`] around the entire [`crate::executor::Runtime::run`]
/// call, not inside individual logical threads: native context guards cannot span logical task
/// switches.
#[track_caller]
pub fn spawn_thread<F, T>(name: &str, f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(not(feature = "fuzz"))]
    let handle = Handle::try_current().ok();
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            #[cfg(not(feature = "fuzz"))]
            let _guard = handle.as_ref().map(Handle::enter);
            f()
        })
        .unwrap_or_else(|e| panic!("failed to spawn thread {name:?}: {e}"))
}

/// Spawns a scoped thread on the selected blocking backend.
///
/// This is the scoped version of [`spawn_thread`], for use with [`thread::scope`], with the same
/// Tokio context rules: native OS threads enter the captured handle, while `fuzz` logical threads
/// share the context entered around the entire [`crate::executor::Runtime::run`] call.
#[track_caller]
pub fn spawn_scoped_thread<'scope, 'env, F, T>(
    scope: &'scope thread::Scope<'scope, 'env>,
    name: &str,
    f: F,
) -> thread::ScopedJoinHandle<'scope, T>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    #[cfg(not(feature = "fuzz"))]
    let handle = Handle::try_current().ok();
    thread::Builder::new()
        .name(name.to_string())
        .spawn_scoped(scope, move || {
            #[cfg(not(feature = "fuzz"))]
            let _guard = handle.as_ref().map(Handle::enter);
            f()
        })
        .unwrap_or_else(|e| panic!("failed to spawn scoped thread {name:?}: {e}"))
}

/// Spawns a blocking closure on the current tokio runtime, like [`tokio::task::spawn_blocking`].
///
/// The closure is boxed first, so tokio's blocking task machinery is instantiated once per return
/// type `R` instead of once per closure type.
///
/// # Panics
///
/// Panics if called outside of a tokio runtime context.
pub fn spawn_blocking<F, R>(func: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(Box::new(func) as Box<dyn FnOnce() -> R + Send>)
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
    /// How many [`GracefulShutdown`](crate::shutdown::GracefulShutdown) tasks are currently
    /// active.
    graceful_tasks: Arc<AtomicUsize>,
}

// === impl TaskManager ===

impl TaskManager {
    /// Create a new [`TaskManager`] without an associated [`Runtime`], returning
    /// the shutdown/event primitives for [`crate::RuntimeBuilder`] to wire up.
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
    pub fn graceful_shutdown_with_timeout(self, timeout: time::Duration) -> bool {
        self.do_graceful_shutdown(Some(timeout))
    }

    fn do_graceful_shutdown(self, timeout: Option<time::Duration>) -> bool {
        drop(self.signal);
        let deadline = timeout.map(|t| time::Instant::now() + t);
        while self.graceful_tasks.load(Ordering::SeqCst) > 0 {
            if deadline.is_some_and(|d| time::Instant::now() > d) {
                debug!("graceful shutdown timed out");
                return false;
            }
            wait_for_shutdown_progress();
        }
        debug!("gracefully shut down");
        true
    }
}

pub(crate) fn wait_for_shutdown_progress() {
    #[cfg(not(feature = "fuzz"))]
    thread::yield_now();
    // A runnable yield loop prevents the virtual clock from reaching the shutdown deadline.
    #[cfg(feature = "fuzz")]
    time::sleep(time::Duration::from_millis(1));
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
    pub(super) task_name: &'static str,
    pub(super) error: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

    #[test]
    fn test_critical() {
        let rt = Runtime::test();
        let handle = rt.take_task_manager_handle().unwrap();

        rt.spawn_critical_task("this is a critical task", async { panic!("intentionally panic") });

        rt.handle().block_on(async move {
            let err_result = handle.await.unwrap();
            assert!(err_result.is_err(), "Expected TaskManager to return an error due to panic");
            let panicked_err = err_result.unwrap_err();

            assert_eq!(panicked_err.task_name, "this is a critical task");
            assert_eq!(panicked_err.error, Some("intentionally panic".to_string()));
        })
    }

    #[test]
    fn test_manager_shutdown_critical() {
        let rt = Runtime::test();

        let (signal, shutdown) = signal();

        rt.spawn_critical_task("this is a critical task", async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        });

        rt.graceful_shutdown();

        rt.handle().block_on(shutdown);
    }

    #[test]
    fn test_manager_shutdown() {
        let rt = Runtime::test();

        let (signal, shutdown) = signal();

        rt.spawn_task(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(signal);
        });

        rt.graceful_shutdown();

        rt.handle().block_on(shutdown);
    }

    #[test]
    fn test_manager_graceful_shutdown() {
        let rt = Runtime::test();

        let val = Arc::new(AtomicBool::new(false));
        let c = val.clone();
        rt.spawn_critical_with_graceful_shutdown_signal("grace", async move |shutdown| {
            let _guard = shutdown.await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            c.store(true, Ordering::Relaxed);
        });

        rt.graceful_shutdown();
        assert!(val.load(Ordering::Relaxed));
    }

    #[test]
    fn test_manager_graceful_shutdown_many() {
        let rt = Runtime::test();

        let counter = Arc::new(AtomicUsize::new(0));
        let num = 10;
        for _ in 0..num {
            let c = counter.clone();
            rt.spawn_critical_with_graceful_shutdown_signal("grace", async move |shutdown| {
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
        let rt = Runtime::test();

        let timeout = Duration::from_millis(500);
        let val = Arc::new(AtomicBool::new(false));
        let val2 = val.clone();
        rt.spawn_critical_with_graceful_shutdown_signal("grace", async move |shutdown| {
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
        let rt = Runtime::test();
        let _handle = rt.handle();
    }

    #[test]
    fn test_graceful_shutdown_triggered_by_executor() {
        let rt = Runtime::test();
        let task_manager_handle = rt.take_task_manager_handle().unwrap();

        let task_did_shutdown_flag = Arc::new(AtomicBool::new(false));
        let flag_clone = task_did_shutdown_flag.clone();

        let spawned_task_handle = rt.spawn_with_signal(async move |shutdown_signal| {
            shutdown_signal.await;
            flag_clone.store(true, Ordering::SeqCst);
        });

        let send_result = rt.initiate_graceful_shutdown();
        assert!(send_result.is_ok());

        let manager_final_result = rt.handle().block_on(task_manager_handle);
        assert!(manager_final_result.is_ok(), "TaskManager task should not panic");
        assert_eq!(manager_final_result.unwrap(), Ok(()));

        let task_join_result = rt.handle().block_on(spawned_task_handle);
        assert!(task_join_result.is_ok());

        assert!(task_did_shutdown_flag.load(Ordering::Relaxed));
    }
}
