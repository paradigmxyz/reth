#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth task management

use futures_util::{Future, FutureExt, Stream};
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
}

// === impl TaskManager ===

impl TaskManager {
    /// Create a new instance connected to the given handle's tokio runtime.
    pub fn new(handle: Handle) -> Self {
        let (panicked_tasks_tx, panicked_tasks_rx) = unbounded_channel();
        Self { handle, panicked_tasks_tx, panicked_tasks_rx }
    }

    /// Returns a new [`TaskExecutor`] that can spawn new tasks onto the tokio runtime this type is
    /// connected to.
    pub fn executor(&self) -> TaskExecutor {
        TaskExecutor {
            handle: self.handle.clone(),
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
        let task = async move { fut.await }.in_current_span();
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

        // wrap the task in catch unwind
        let task = std::panic::AssertUnwindSafe(fut)
            .catch_unwind()
            .map(move |res| {
                error!("Critical task `{name}` panicked: {res:?}");
                let _ = panicked_tasks_tx.send(name.to_string());
            })
            .in_current_span();
        self.handle.spawn(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

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
}
