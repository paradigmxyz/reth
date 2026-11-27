//! A map of named single-thread worker pools.
//!
//! Each worker is a dedicated OS thread that processes closures sent to it via a channel.
//! This is a substitute for `spawn_blocking` that reuses the same OS thread for the same
//! named task, like a 1-thread thread pool keyed by name. Under `fuzz` plus `--cfg dst`, workers
//! use the deterministic executor and work queues use the selected channel backend.
//! Task results still use Tokio oneshot channels.

use crate::metrics::WorkerThreadMetrics;
use dashmap::DashMap;
use reth_concurrency::sync::{
    atomic::{AtomicUsize, Ordering},
    thread,
    time::Instant,
};
use std::{panic::AssertUnwindSafe, sync::Arc};
use tokio::sync::oneshot;

#[cfg(not(all(feature = "fuzz", dst)))]
use tokio::sync::mpsc;

#[cfg(all(feature = "fuzz", dst))]
use reth_concurrency::sync::channel;

type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

/// A single-thread worker that processes closures sequentially on a dedicated OS thread.
struct WorkerThread {
    /// Sender to submit work to this worker's thread.
    #[cfg(not(all(feature = "fuzz", dst)))]
    tx: mpsc::UnboundedSender<BoxedTask>,
    #[cfg(all(feature = "fuzz", dst))]
    tx: Option<channel::Sender<BoxedTask>>,
    /// Metrics labeled with this worker's name.
    metrics: WorkerThreadMetrics,
    /// Number of tasks currently running or queued on this worker.
    pending: Arc<AtomicUsize>,
    /// The OS thread handle. Taken during shutdown to join.
    handle: Option<thread::JoinHandle<()>>,
}

impl WorkerThread {
    /// Spawns a new worker thread with the given name.
    fn new(name: &'static str) -> Self {
        #[cfg(not(all(feature = "fuzz", dst)))]
        let (tx, mut rx) = mpsc::unbounded_channel::<BoxedTask>();
        // The selected queue yields to the simulator; Tokio's blocking receive would
        // park the simulator's only OS thread.
        #[cfg(all(feature = "fuzz", dst))]
        let (tx, rx) = channel::unbounded::<BoxedTask>();
        let handle = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                while let Some(task) = {
                    #[cfg(not(all(feature = "fuzz", dst)))]
                    {
                        rx.blocking_recv()
                    }
                    #[cfg(all(feature = "fuzz", dst))]
                    {
                        rx.recv().ok()
                    }
                } {
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(task));
                }
            })
            .unwrap_or_else(|e| panic!("failed to spawn worker thread {name:?}: {e}"));

        Self {
            #[cfg(not(all(feature = "fuzz", dst)))]
            tx,
            #[cfg(all(feature = "fuzz", dst))]
            tx: Some(tx),
            metrics: WorkerThreadMetrics::new(name),
            pending: Arc::new(AtomicUsize::new(0)),
            handle: Some(handle),
        }
    }

    /// Spawns a closure on this worker.
    fn spawn<F, R>(&self, f: F) -> oneshot::Receiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.pending.fetch_add(1, Ordering::AcqRel);

        let (result_tx, result_rx) = oneshot::channel();
        let pending = self.pending.clone();
        let metrics = self.metrics.clone();
        let queued_at = Instant::now();

        let task: BoxedTask = Box::new(move || {
            let started_at = Instant::now();
            metrics.record_queue_wait(started_at.saturating_duration_since(queued_at));
            let result = {
                let _decrement_pending = DecrementPendingOnDrop(pending);
                let _record_task_duration = RecordTaskDurationOnDrop::new(metrics, started_at);
                f()
            };
            let _ = result_tx.send(result);
        });

        #[cfg(not(all(feature = "fuzz", dst)))]
        let tx = &self.tx;
        #[cfg(all(feature = "fuzz", dst))]
        let tx = self.tx.as_ref().expect("live worker retains its sender");
        if tx.send(task).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }

        result_rx
    }

    /// Attempts to spawn a closure if this worker has no task running or queued.
    fn try_spawn<F, R>(&self, f: F) -> Option<oneshot::Receiver<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.pending.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).ok()?;

        let (result_tx, result_rx) = oneshot::channel();
        let pending = self.pending.clone();
        let metrics = self.metrics.clone();
        let queued_at = Instant::now();

        let task: BoxedTask = Box::new(move || {
            let started_at = Instant::now();
            metrics.record_queue_wait(started_at.saturating_duration_since(queued_at));
            let result = {
                let _decrement_pending = DecrementPendingOnDrop(pending);
                let _record_task_duration = RecordTaskDurationOnDrop::new(metrics, started_at);
                f()
            };
            let _ = result_tx.send(result);
        });

        #[cfg(not(all(feature = "fuzz", dst)))]
        let tx = &self.tx;
        #[cfg(all(feature = "fuzz", dst))]
        let tx = self.tx.as_ref().expect("live worker retains its sender");
        if tx.send(task).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return None
        }

        Some(result_rx)
    }
}

/// Decrements a worker's pending task count when a task finishes, including after panic.
struct DecrementPendingOnDrop(Arc<AtomicUsize>);

impl Drop for DecrementPendingOnDrop {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Records a worker task's run time when the task finishes or unwinds.
struct RecordTaskDurationOnDrop {
    metrics: WorkerThreadMetrics,
    started_at: Instant,
}

impl RecordTaskDurationOnDrop {
    const fn new(metrics: WorkerThreadMetrics, started_at: Instant) -> Self {
        Self { metrics, started_at }
    }
}

impl Drop for RecordTaskDurationOnDrop {
    fn drop(&mut self) {
        self.metrics.record_task_duration(self.started_at.elapsed());
    }
}

/// A map of named single-thread workers.
///
/// Each unique name gets a dedicated OS thread that is reused for all tasks submitted under
/// that name. Workers are created lazily on first use.
pub(crate) struct WorkerMap {
    #[cfg(not(all(feature = "fuzz", dst)))]
    workers: DashMap<&'static str, WorkerThread>,
    #[cfg(all(feature = "fuzz", dst))]
    workers: DashMap<&'static str, Arc<WorkerThread>>,
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

    /// Returns the canonical worker without retaining map locks across scheduled operations.
    #[cfg(all(feature = "fuzz", dst))]
    fn worker(&self, name: &'static str) -> Arc<WorkerThread> {
        if let Some(worker) = self.workers.get(name) {
            return Arc::clone(worker.value());
        }
        // Creating or dropping a worker can yield. Neither may hold a DashMap guard.
        let candidate = Arc::new(WorkerThread::new(name));
        let worker =
            Arc::clone(self.workers.entry(name).or_insert_with(|| Arc::clone(&candidate)).value());
        drop(candidate);
        worker
    }

    /// Spawns a closure on the dedicated worker thread for the given name.
    ///
    /// If no worker thread exists for this name yet, one is created with the given name as
    /// the OS thread name. The closure executes on the worker's OS thread and the returned
    /// receiver resolves with the result.
    pub(crate) fn spawn_on<F, R>(&self, name: &'static str, f: F) -> oneshot::Receiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(not(all(feature = "fuzz", dst)))]
        let worker = self.workers.entry(name).or_insert_with(|| WorkerThread::new(name));
        #[cfg(all(feature = "fuzz", dst))]
        let worker = self.worker(name);
        worker.spawn(f)
    }

    /// Attempts to spawn a closure on the dedicated worker thread for the given name.
    ///
    /// Returns `None` if the named worker already has a task running or queued.
    pub(crate) fn try_spawn_on<F, R>(
        &self,
        name: &'static str,
        f: F,
    ) -> Option<oneshot::Receiver<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(not(all(feature = "fuzz", dst)))]
        let worker = self.workers.entry(name).or_insert_with(|| WorkerThread::new(name));
        #[cfg(all(feature = "fuzz", dst))]
        let worker = self.worker(name);
        worker.try_spawn(f)
    }
}

#[cfg(not(all(feature = "fuzz", dst)))]
impl Drop for WorkerMap {
    fn drop(&mut self) {
        for (_, mut w) in std::mem::take(&mut self.workers) {
            // Drop sender so the thread's recv loop exits, then join.
            drop(w.tx);
            // A task can own the last reference to this map and drop it on its own worker, which
            // cannot join itself. That thread exits on its own once the task returns.
            if let Some(handle) = w.handle.take() &&
                handle.thread().id() != thread::current().id()
            {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Drop for WorkerThread {
    fn drop(&mut self) {
        // Close the queue before joining so an idle worker can exit.
        drop(self.tx.take());
        // The last map owner may be dropped by this worker's own task.
        if let Some(handle) = self.handle.take() &&
            handle.thread().id() != thread::current().id()
        {
            let _ = handle.join();
        }
    }
}

impl std::fmt::Debug for WorkerMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerMap").field("num_workers", &self.workers.len()).finish()
    }
}

// Native Tokio waits do not yield to Shuttle while a simulated worker is parked.
#[cfg(all(test, not(all(feature = "fuzz", dst))))]
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

    #[tokio::test]
    async fn worker_map_try_spawn_busy() {
        let map = WorkerMap::new();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let first = map.try_spawn_on("busy-worker", move || {
            release_rx.recv().unwrap();
            1
        });
        assert!(first.is_some());

        let second = map.try_spawn_on("busy-worker", || 2);
        assert!(second.is_none(), "busy worker should reject queued work");

        release_tx.send(()).unwrap();
        assert_eq!(first.unwrap().await.unwrap(), 1);

        let third = map.try_spawn_on("busy-worker", || 3).expect("worker should be idle");
        assert_eq!(third.await.unwrap(), 3);
    }

    #[test]
    fn worker_map_last_owner_dropped_on_worker() {
        let map = Arc::new(WorkerMap::new());
        let owner = Arc::clone(&map);
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let done = map.spawn_on("last-owner", move || {
            release_rx.recv().unwrap();
            // Drops the last reference to the map on the worker it would join.
            drop(owner);
        });

        drop(map);
        release_tx.send(()).unwrap();
        done.blocking_recv().expect("dropping the map on its own worker should not panic");
    }
}
