//! Persistent proof worker pool with session-based execution.
//!
//! This module provides a long-lived worker pool that persists across payloads,
//! eliminating thread spawn overhead on the critical execution path.
//!
//! # Architecture
//!
//! - [`ProofWorkerPool`]: Created once at node startup, spawns dedicated OS threads
//! - Workers wait for "session" closures that capture the concrete factory type
//! - Per session: closure creates provider, drains jobs, then returns
//! - Threads persist and wait for the next session
//!
//! # Design
//!
//! The challenge is that `Factory` types vary per payload (different `OverlayStateProviderFactory`
//! configurations). To make workers persistent while factory types change, we use **session
//! closures**: each session sends a `Box<dyn FnOnce() + Send>` to workers that captures the
//! concrete factory and job channels, runs the drain loop with concrete types, then returns.
//!
//! This avoids object-safety issues with `TrieCursorFactory` and `HashedCursorFactory` which
//! have associated types with lifetimes.

use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};
use tracing::{debug, trace};

/// A closure that runs a proof session on a worker thread.
///
/// The closure captures the concrete factory type and job channels,
/// creates a fresh database provider, drains jobs, then returns.
pub type SessionRunner = Box<dyn FnOnce(usize) + Send + 'static>;

/// Control message for persistent worker threads.
enum WorkerMessage {
    /// Run a session with the given closure.
    RunSession(SessionRunner),
    /// Shutdown the worker.
    Shutdown,
}

/// A persistent pool of proof worker threads.
///
/// Created once at node startup. Workers persist across payloads and receive
/// session closures that capture the per-payload factory and job channels.
#[derive(Debug)]
pub struct ProofWorkerPool {
    /// Senders to dispatch sessions to workers.
    worker_txs: Vec<Sender<WorkerMessage>>,
    /// Worker thread handles for shutdown.
    worker_handles: Vec<JoinHandle<()>>,
    /// Flag indicating the pool is shutting down.
    shutdown: Arc<AtomicBool>,
}

impl ProofWorkerPool {
    /// Creates a new persistent proof worker pool.
    ///
    /// Spawns `worker_count` dedicated OS threads that wait for session closures.
    /// Threads are named `proof-worker-N` for debugging.
    pub fn new(worker_count: usize) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut worker_txs = Vec::with_capacity(worker_count);
        let mut worker_handles = Vec::with_capacity(worker_count);

        debug!(
            target: "trie::worker_pool",
            worker_count,
            "Creating persistent proof worker pool"
        );

        for worker_id in 0..worker_count {
            // Use bounded channel with capacity 1 - workers process one session at a time
            let (tx, rx) = bounded::<WorkerMessage>(1);
            worker_txs.push(tx);

            let shutdown_clone = shutdown.clone();
            let handle = thread::Builder::new()
                .name(format!("proof-worker-{worker_id}"))
                .spawn(move || {
                    worker_loop(worker_id, rx, shutdown_clone);
                })
                .expect("failed to spawn proof worker thread");

            worker_handles.push(handle);
        }

        Self { worker_txs, worker_handles, shutdown }
    }

    /// Returns the number of workers in the pool.
    pub fn worker_count(&self) -> usize {
        self.worker_txs.len()
    }

    /// Dispatches a session closure to a specific worker.
    ///
    /// The closure should capture the factory, job channels, and any other
    /// per-session state, then run the job drain loop.
    ///
    /// Returns `Err` if the worker has shut down.
    pub fn dispatch_session(
        &self,
        worker_id: usize,
        session: SessionRunner,
    ) -> Result<(), crossbeam_channel::SendError<()>> {
        self.worker_txs
            .get(worker_id)
            .ok_or(crossbeam_channel::SendError(()))?
            .send(WorkerMessage::RunSession(session))
            .map_err(|_| crossbeam_channel::SendError(()))
    }

    /// Dispatches session closures to all workers.
    ///
    /// `session_factory` is called once per worker to create the session closure.
    /// This allows each worker to have its own job receiver clone.
    pub fn dispatch_all<F>(
        &self,
        mut session_factory: F,
    ) -> Result<(), crossbeam_channel::SendError<()>>
    where
        F: FnMut(usize) -> SessionRunner,
    {
        for (worker_id, tx) in self.worker_txs.iter().enumerate() {
            let session = session_factory(worker_id);
            tx.send(WorkerMessage::RunSession(session))
                .map_err(|_| crossbeam_channel::SendError(()))?;
        }
        Ok(())
    }

    /// Shuts down the worker pool gracefully.
    ///
    /// Sends shutdown messages to all workers and waits for them to finish.
    pub fn shutdown(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Send shutdown to all workers
        for tx in &self.worker_txs {
            let _ = tx.send(WorkerMessage::Shutdown);
        }

        // Wait for all workers to finish
        for handle in self.worker_handles.drain(..) {
            let _ = handle.join();
        }

        debug!(
            target: "trie::worker_pool",
            "Proof worker pool shut down"
        );
    }
}

impl Drop for ProofWorkerPool {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Send shutdown to all workers
        for tx in &self.worker_txs {
            let _ = tx.send(WorkerMessage::Shutdown);
        }

        // Wait for all workers - need to drain handles
        for handle in self.worker_handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// Main loop for a persistent worker thread.
fn worker_loop(worker_id: usize, rx: Receiver<WorkerMessage>, shutdown: Arc<AtomicBool>) {
    trace!(
        target: "trie::worker_pool",
        worker_id,
        "Worker started"
    );

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        match rx.recv() {
            Ok(WorkerMessage::RunSession(session)) => {
                trace!(
                    target: "trie::worker_pool",
                    worker_id,
                    "Starting session"
                );

                // Run the session closure - it captures the factory and drains jobs
                session(worker_id);

                trace!(
                    target: "trie::worker_pool",
                    worker_id,
                    "Session complete"
                );
            }
            Ok(WorkerMessage::Shutdown) => {
                trace!(
                    target: "trie::worker_pool",
                    worker_id,
                    "Received shutdown"
                );
                break;
            }
            Err(_) => {
                // Channel closed, shutdown
                break;
            }
        }
    }

    trace!(
        target: "trie::worker_pool",
        worker_id,
        "Worker exiting"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_worker_pool_basic() {
        let pool = ProofWorkerPool::new(2);
        assert_eq!(pool.worker_count(), 2);

        let counter = Arc::new(AtomicUsize::new(0));

        // Dispatch sessions to both workers
        let counter_clone = counter.clone();
        pool.dispatch_all(|_worker_id| {
            let counter = counter_clone.clone();
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        })
        .unwrap();

        // Give workers time to run
        std::thread::sleep(std::time::Duration::from_millis(100));

        assert_eq!(counter.load(Ordering::SeqCst), 2);

        pool.shutdown();
    }

    #[test]
    fn test_worker_pool_multiple_sessions() {
        let pool = ProofWorkerPool::new(1);
        let counter = Arc::new(AtomicUsize::new(0));

        // Run 3 sessions on the same worker
        for _ in 0..3 {
            let counter_clone = counter.clone();
            pool.dispatch_session(
                0,
                Box::new(move |_| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert_eq!(counter.load(Ordering::SeqCst), 3);

        pool.shutdown();
    }
}
