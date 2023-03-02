#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth task management

use crate::shutdown::{signal, Shutdown, Signal};
use dyn_clone::DynClone;
use futures_util::{
    future::{select, BoxFuture},
    pin_mut, Future, FutureExt, TryFutureExt,
};
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use tracing::error;
use tracing_futures::Instrument;

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
///  use reth_tasks::{TaskSpawner, TokioTaskExecutor};
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
pub struct TaskManager {
    /// Handle to the tokio runtime this task manager is associated with.
    ///
    /// See [`Handle`] docs.
    handle: Handle,
    /// Sender half for sending panic signals to this type
    panicked_tasks_tx: UnboundedSender<&'static str>,
    /// Listens for panicked tasks
    panicked_tasks_rx: UnboundedReceiver<&'static str>,
    /// The [Signal] to fire when all tasks should be shutdown.
    ///
    /// This is fired on drop.
    _signal: Signal,
    /// Receiver of the shutdown signal.
    on_shutdown: Shutdown,
}

// === impl TaskManager ===

impl TaskManager {
    /// Create a new instance connected to the given handle's tokio runtime.
    pub fn new(handle: Handle) -> Self {
        let (panicked_tasks_tx, panicked_tasks_rx) = unbounded_channel();
        let (_signal, on_shutdown) = signal();
        Self { handle, panicked_tasks_tx, panicked_tasks_rx, _signal, on_shutdown }
    }

    /// Returns a new [`TaskExecutor`] that can spawn new tasks onto the tokio runtime this type is
    /// connected to.
    pub fn executor(&self) -> TaskExecutor {
        TaskExecutor {
            handle: self.handle.clone(),
            on_shutdown: self.on_shutdown.clone(),
            panicked_tasks_tx: self.panicked_tasks_tx.clone(),
        }
    }
}

/// An endless future that resolves if a critical task panicked.
///
/// See [`TaskExecutor::spawn_critical`]
impl Future for TaskManager {
    type Output = PanickedTaskError;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let err = ready!(self.get_mut().panicked_tasks_rx.poll_recv(cx));
        Poll::Ready(err.map(PanickedTaskError).expect("stream can not end"))
    }
}

/// Error with the name of the task that panicked.
#[derive(Debug, thiserror::Error)]
#[error("Critical task panicked {0}")]
pub struct PanickedTaskError(&'static str);

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
    panicked_tasks_tx: UnboundedSender<&'static str>,
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

        let task = async move {
            pin_mut!(fut);
            let _ = select(on_shutdown, fut).await;
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
            .inspect_err(move |res| {
                error!("Critical task `{name}` panicked: {res:?}");
                let _ = panicked_tasks_tx.send(name);
            })
            .in_current_span();

        let task = async move {
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
    pub fn spawn_critical_with_signal<F>(
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
            .inspect_err(move |res| {
                error!("Critical task `{name}` panicked: {res:?}");
                let _ = panicked_tasks_tx.send(name);
            })
            .map(|_| ())
            .in_current_span();

        self.handle.spawn(task)
    }
}

impl TaskSpawner for TaskExecutor {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> JoinHandle<()> {
        self.spawn(fut)
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
    use std::time::Duration;

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

        executor.spawn_critical(
            "this is a critical task",
            Box::pin(async { panic!("intentionally panic") }),
        );

        runtime.block_on(async move {
            let err = manager.await;
            assert_eq!(err.0, "this is a critical task");
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

        executor.spawn_critical(
            "this is a critical task",
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                drop(signal);
            }),
        );

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
}
