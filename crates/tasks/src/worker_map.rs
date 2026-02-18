//! A map of named single-thread worker pools.
//!
//! Each worker is a dedicated OS thread that processes closures sent to it via a channel.
//! This is a substitute for `spawn_blocking` that reuses the same OS thread for the same
//! named task, like a 1-thread thread pool keyed by name.

use dashmap::DashMap;
use std::thread;
use tokio::sync::{mpsc, oneshot};

type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

/// A single-thread worker that processes closures sequentially on a dedicated OS thread.
struct WorkerThread {
    /// Sender to submit work to this worker's thread.
    tx: mpsc::UnboundedSender<BoxedTask>,
    /// The OS thread handle.
    _handle: thread::JoinHandle<()>,
}

impl WorkerThread {
    /// Spawns a new worker thread with the given name.
    fn new(name: &'static str) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<BoxedTask>();
        let handle = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                while let Some(task) = rx.blocking_recv() {
                    task();
                }
            })
            .unwrap_or_else(|e| panic!("failed to spawn worker thread {name:?}: {e}"));

        Self { tx, _handle: handle }
    }
}

/// A map of named single-thread workers.
///
/// Each unique name gets a dedicated OS thread that is reused for all tasks submitted under
/// that name. Workers are created lazily on first use.
pub(crate) struct WorkerMap {
    workers: DashMap<&'static str, WorkerThread>,
}

impl Default for WorkerMap {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerMap {
    /// Creates a new empty `WorkerMap`.
    pub(crate) fn new() -> Self {
        Self { workers: DashMap::new() }
    }

    /// Spawns a closure on the dedicated worker thread for the given name.
    ///
    /// If no worker thread exists for this name yet, one is created with the given name as
    /// the OS thread name. The closure executes on the worker's OS thread and the returned
    /// future resolves with the result.
    pub(crate) fn spawn_on<F, R>(&self, name: &'static str, f: F) -> oneshot::Receiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (result_tx, result_rx) = oneshot::channel();

        let task: BoxedTask = Box::new(move || {
            let _ = result_tx.send(f());
        });

        let worker = self.workers.entry(name).or_insert_with(|| WorkerThread::new(name));
        let _ = worker.tx.send(task);

        result_rx
    }
}

impl std::fmt::Debug for WorkerMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerMap").field("num_workers", &self.workers.len()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_map_basic() {
        let map = WorkerMap::new();

        let result = map.spawn_on("test", || 42).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn worker_map_same_thread() {
        let map = WorkerMap::new();

        let id1 = map.spawn_on("test", || thread::current().id()).await.unwrap();
        let id2 = map.spawn_on("test", || thread::current().id()).await.unwrap();
        assert_eq!(id1, id2, "same name should run on the same thread");
    }

    #[tokio::test]
    async fn worker_map_different_names_different_threads() {
        let map = WorkerMap::new();

        let id1 = map.spawn_on("worker-a", || thread::current().id()).await.unwrap();
        let id2 = map.spawn_on("worker-b", || thread::current().id()).await.unwrap();
        assert_ne!(id1, id2, "different names should run on different threads");
    }

    #[tokio::test]
    async fn worker_map_sequential_execution() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let map = WorkerMap::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let mut receivers = Vec::new();
        for i in 0..10 {
            let c = counter.clone();
            let rx = map.spawn_on("sequential", move || {
                let val = c.fetch_add(1, Ordering::SeqCst);
                assert_eq!(val, i, "tasks should execute in order");
                val
            });
            receivers.push(rx);
        }

        for (i, rx) in receivers.into_iter().enumerate() {
            let val = rx.await.unwrap();
            assert_eq!(val, i);
        }
    }

    #[tokio::test]
    async fn worker_map_thread_name() {
        let map = WorkerMap::new();

        let name = map
            .spawn_on("custom-worker", || thread::current().name().unwrap().to_string())
            .await
            .unwrap();
        assert_eq!(name, "custom-worker");
    }
}
