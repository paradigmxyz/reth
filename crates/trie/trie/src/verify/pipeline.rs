//! Storage scheduling, completed-root buffering, shared findings, and background task lifetime.

use super::{Output, StorageWorker};
use crate::{hashed_cursor::HashedStorageCursor, trie_cursor::TrieStorageCursor};
use alloy_primitives::{map::B256Map, B256, U256};
use crossbeam_channel::{Receiver, Sender};
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use std::{
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    thread::{self, JoinHandle},
};

/// Owns background work, buffers completed roots, and merges findings for the account consumer.
#[derive(Debug)]
pub(super) struct StoragePipeline {
    roots_rx: Receiver<(B256, B256)>,
    roots: B256Map<B256>,
    // Held so `events_rx` never disconnects, even after every background thread has exited.
    _events_tx: Sender<Result<Output, StateRootError>>,
    events_rx: Receiver<Result<Output, StateRootError>>,
    // Disconnecting the sole sender broadcasts cancellation to every worker.
    cancel: Option<Sender<()>>,
    background: Option<JoinHandle<()>>,
}

impl StoragePipeline {
    /// Spawns the background thread which walks `addresses` and verifies their storage on one
    /// thread per worker.
    ///
    /// # Panics
    ///
    /// Panics if `workers` is empty.
    pub(super) fn spawn<L, T, H, TX>(
        addresses: L,
        workers: Vec<StorageWorker<T, H, TX>>,
    ) -> Result<Self, DatabaseError>
    where
        L: Iterator<Item = Result<B256, DatabaseError>> + Send + 'static,
        T: TrieStorageCursor + Send + 'static,
        H: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        assert!(!workers.is_empty(), "verification requires storage workers");
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let (roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let scheduler = StorageScheduler {
            addresses,
            workers,
            roots: roots_tx,
            context: WorkerContext { events: events_tx.clone(), cancel: cancel_rx },
        };
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-lookahead".into())
            .spawn(move || {
                let _entered = span.enter();
                scheduler.run();
            })
            .map_err(|error| DatabaseError::Custom(Arc::new(error)))?;
        Ok(Self {
            roots_rx,
            roots: Default::default(),
            _events_tx: events_tx,
            events_rx,
            cancel: Some(cancel_tx),
            background: Some(background),
        })
    }

    /// Blocks until the storage root of `address` is computed, buffering roots of other accounts
    /// and moving findings into `outputs` while waiting.
    ///
    /// `address` must have non-empty storage, and each root can only be taken once.
    pub(super) fn root(
        &mut self,
        address: B256,
        outputs: &mut VecDeque<Result<Output, StateRootError>>,
    ) -> Result<B256, StateRootError> {
        if let Some(root) = self.roots.remove(&address) {
            return Ok(root)
        }
        loop {
            crossbeam_channel::select! {
                // Never disconnects, see `_events_tx`.
                recv(self.events_rx) -> event => {
                    if let Ok(event) = event { outputs.push_back(Ok(event?)); }
                }
                recv(self.roots_rx) -> root => {
                    let Ok((root_address, root)) = root else {
                        self.drain(outputs)?;
                        return Err(DatabaseError::Other(format!(
                            "storage root of {address} was never computed"
                        ))
                        .into())
                    };
                    if root_address == address { return Ok(root) }
                    self.roots.insert(root_address, root);
                }
            }
        }
    }

    /// Moves already received findings into `outputs` without blocking, stopping at the first
    /// error.
    pub(super) fn drain(
        &self,
        outputs: &mut VecDeque<Result<Output, StateRootError>>,
    ) -> Result<(), StateRootError> {
        for event in self.events_rx.try_iter() {
            outputs.push_back(Ok(event?));
        }
        Ok(())
    }

    /// Appends `found` to `outputs` after any findings already received from background threads.
    pub(super) fn report(
        &self,
        found: Vec<Output>,
        outputs: &mut VecDeque<Result<Output, StateRootError>>,
    ) -> Result<(), StateRootError> {
        self.drain(outputs)?;
        outputs.extend(found.into_iter().map(Ok));
        Ok(())
    }

    /// Waits for all scheduled work to complete; queued findings and failures remain available to
    /// drain.
    pub(super) fn finish(&mut self) {
        self.join();
    }

    /// Cancels outstanding work and waits for background threads to exit.
    pub(super) fn stop(&mut self) {
        self.cancel.take();
        self.join();
    }

    fn join(&mut self) {
        // Re-raising while already unwinding would abort the process.
        if let Some(background) = self.background.take() &&
            let Err(panic) = background.join() &&
            !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for StoragePipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Body of the background thread: walks storage addresses and keeps the workers supplied with
/// jobs.
struct StorageScheduler<L, T, H, TX> {
    addresses: L,
    workers: Vec<StorageWorker<T, H, TX>>,
    roots: Sender<(B256, B256)>,
    context: WorkerContext,
}

impl<L, T, H, TX> StorageScheduler<L, T, H, TX>
where
    L: Iterator<Item = Result<B256, DatabaseError>>,
    T: TrieStorageCursor + Send,
    H: HashedStorageCursor<Value = U256> + Send,
    TX: Send,
{
    fn run(mut self) {
        let context = self.context.clone();
        context.run(|| {
            let worker_count = self.workers.len();
            let pool = reth_tasks::WorkerPool::new(worker_count, "trie-verify");
            // Only queued work is bounded; completed roots never wait for the account thread.
            let (jobs_tx, jobs_rx) = crossbeam_channel::bounded(worker_count * 2);
            pool.in_place_scope(|scope| {
                for worker in self.workers.drain(..) {
                    let jobs = jobs_rx.clone();
                    let roots = self.roots.clone();
                    let context = self.context.clone();
                    let span = tracing::Span::current();
                    scope.spawn(move |_| {
                        let _entered = span.enter();
                        context.run(|| worker.run(jobs, &roots, &context));
                    });
                }
                context.run(|| self.produce(jobs_tx));
            });
            Ok(())
        });
    }

    fn produce(&mut self, jobs: Sender<B256>) -> Result<(), StateRootError> {
        for address in self.addresses.by_ref() {
            if !self.context.send(&jobs, address?) {
                return Ok(())
            }
        }
        Ok(())
    }
}

/// Shared cancellation and reporting policy for background threads.
///
/// Verification is cancelled once the `cancel` sender is dropped; nothing is ever sent on it.
#[derive(Clone, Debug)]
pub(super) struct WorkerContext {
    events: Sender<Result<Output, StateRootError>>,
    cancel: Receiver<()>,
}

impl WorkerContext {
    /// Sends `value`, returning `false` if it was not sent because verification was cancelled or
    /// `sender` disconnected.
    pub(super) fn send<T>(&self, sender: &Sender<T>, value: T) -> bool {
        crossbeam_channel::select! {
            send(sender, value) -> result => result.is_ok(),
            recv(self.cancel) -> _ => false,
        }
    }

    /// Receives the next job. Returns `None` once `jobs` is exhausted, and may return `None` early
    /// if verification was cancelled.
    pub(super) fn receive(&self, jobs: &Receiver<B256>) -> Option<B256> {
        crossbeam_channel::select! {
            recv(jobs) -> job => job.ok(),
            recv(self.cancel) -> _ => None,
        }
    }

    /// Returns whether verification was cancelled.
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancel.try_recv() == Err(crossbeam_channel::TryRecvError::Disconnected)
    }

    /// Sends each finding, returning `false` if one was not sent because verification was
    /// cancelled.
    pub(super) fn report(&self, found: Vec<Output>) -> bool {
        for output in found {
            if !self.send(&self.events, Ok(output)) {
                return false
            }
        }
        true
    }

    /// Runs `work`, reporting its error or panic as a verification error.
    fn run(&self, work: impl FnOnce() -> Result<(), StateRootError>) {
        let result = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            Err(DatabaseError::Other(format!("trie verification thread panicked: {message}"))
                .into())
        });
        // The error is only lost if verification was already cancelled, in which case the
        // verifier has stopped reading results.
        if let Err(error) = result {
            self.send(&self.events, Err(error));
        }
    }
}
