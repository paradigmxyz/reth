#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth task management

use crate::shutdown::{signal, Shutdown, Signal};
use futures_util::{future::select, pin_mut, Future, FutureExt, Stream};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};
use tracing::error;
use tracing_futures::Instrument;

pub mod shutdown;

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
    panicked_tasks_tx: UnboundedSender<String>,
    /// Listens for panicked tasks
    panicked_tasks_rx: UnboundedReceiver<String>,
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

/// A stream that yields the name of panicked tasks.
///
/// See [`TaskExecutor::spawn_critical`]
impl Stream for TaskManager {
    type Item = String;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().panicked_tasks_rx.poll_recv(cx)
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
    panicked_tasks_tx: UnboundedSender<String>,
}

// === impl TaskExecutor ===

impl TaskExecutor {
    /// Spawns the task onto the runtime.
    ///
    /// See also [`Handle::spawn`].
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let on_shutdown = self.on_shutdown.clone();

        let task = async move {
            pin_mut!(fut);
            let _ = select(on_shutdown, fut).await;
        }
        .in_current_span();

        self.handle.spawn(task);
    }

    /// This spawns a critical task onto the runtime.
    ///
    /// If this task panics, the [`TaskManager`] is notified.
    pub fn spawn_critical<F>(&self, name: &'static str, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panicked_tasks_tx = self.panicked_tasks_tx.clone();
        let on_shutdown = self.on_shutdown.clone();

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map(move |res| {
                error!("Critical task `{name}` panicked: {res:?}");
                let _ = panicked_tasks_tx.send(name.to_string());
            })
            .in_current_span();

        self.handle.spawn(async move {
            pin_mut!(task);
            let _ = select(on_shutdown, task).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::time::Duration;

    #[test]
    fn test_critical() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let mut manager = TaskManager::new(handle);
        let executor = manager.executor();

        executor.spawn_critical(
            "this is a critical task",
            Box::pin(async { panic!("intentionally panic") }),
        );

        runtime.block_on(async move {
            let panicked_task = manager.next().await.unwrap();
            assert_eq!(panicked_task, "this is a critical task");
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
