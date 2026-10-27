//! Ordered storage scheduling, root delivery, shared findings, and background task lifetime.

use super::{Output, StorageJob, StorageWorker};
use crate::{hashed_cursor::HashedStorageCursor, trie_cursor::TrieStorageCursor};
use alloy_primitives::{B256, U256};
use crossbeam_channel::{Receiver, Sender};
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    thread::{self, JoinHandle},
};

/// Owns storage scheduling and one pending root announcement for the account producer.
#[derive(Debug)]
pub(super) struct StoragePipeline {
    context: WorkerContext,
    background: Option<JoinHandle<()>>,
    announcements: Receiver<(B256, Receiver<B256>)>,
    next: Option<(B256, Receiver<B256>)>,
}

impl StoragePipeline {
    /// Spawns the background thread which walks `addresses` and verifies their storage on one
    /// thread per worker.
    ///
    /// # Panics
    ///
    /// Panics if `workers` is empty or `storage_root_limit` is zero.
    pub(super) fn spawn<L, T, H, TX>(
        addresses: L,
        workers: Vec<StorageWorker<T, H, TX>>,
        context: WorkerContext,
        storage_root_limit: usize,
    ) -> Result<Self, DatabaseError>
    where
        L: Iterator<Item = Result<StorageJob, DatabaseError>> + Send + 'static,
        T: TrieStorageCursor + Send + 'static,
        H: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        assert!(!workers.is_empty(), "verification requires storage workers");
        assert!(storage_root_limit > 0, "storage root limit must be positive");
        // The account producer retains one announcement outside the queue. Reserving its
        // slot here bounds all dispatched roots, including that lookahead, by the limit.
        let (announcements_tx, announcements) = crossbeam_channel::bounded(storage_root_limit - 1);
        let scheduler = StorageScheduler { addresses, workers, context: context.clone() };
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-lookahead".into())
            .spawn(move || {
                let _entered = span.enter();
                scheduler.run(announcements_tx);
            })
            .map_err(|error| DatabaseError::Custom(Arc::new(error)))?;
        Ok(Self { context, background: Some(background), announcements, next: None })
    }

    /// Returns the next root in address order up to and including `up_to`, or all remaining
    /// roots if `up_to` is `None`. Returns `None` when the bound is reached, scanning completes
    /// without another root, or verification is cancelled. Findings are consumed independently.
    /// A missing worker result waits for cancellation by the findings consumer after it receives
    /// the worker error, rather than replacing the original error with a channel error.
    pub(super) fn next_root(&mut self, up_to: Option<B256>) -> Option<(B256, B256)> {
        if self.context.is_cancelled() {
            return None;
        }
        if self.next.is_none() {
            self.next = crossbeam_channel::select! {
                recv(self.announcements) -> announcement => announcement.ok(),
                recv(self.context.cancel) -> _ => return None,
            };
        }
        let Some((address, root)) = &self.next else { return None };
        if up_to.is_some_and(|bound| *address > bound) {
            return None;
        }
        let root = crossbeam_channel::select! {
            recv(root) -> root => root,
            recv(self.context.cancel) -> _ => return None,
        };
        match root {
            Ok(root) => {
                let address = *address;
                self.next = None;
                Some((address, root))
            }
            Err(_) => {
                // A failing worker drops its root sender before WorkerContext::run reports the
                // original error. The findings consumer cancels us after receiving that error;
                // do not race it with a synthetic channel error or skip the missing root.
                match self.context.cancel.recv() {
                    Ok(()) | Err(_) => None,
                }
            }
        }
    }

    /// Joins storage producers while the caller's iterator continues consuming findings.
    pub(super) fn finish(&mut self) {
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
        self.join();
    }
}

/// Body of the background thread: walks storage addresses and keeps the workers supplied with
/// jobs.
struct StorageScheduler<L, T, H, TX> {
    addresses: L,
    workers: Vec<StorageWorker<T, H, TX>>,
    context: WorkerContext,
}

impl<L, T, H, TX> StorageScheduler<L, T, H, TX>
where
    L: Iterator<Item = Result<StorageJob, DatabaseError>>,
    T: TrieStorageCursor + Send,
    H: HashedStorageCursor<Value = U256> + Send,
    TX: Send,
{
    fn run(mut self, announcements: Sender<(B256, Receiver<B256>)>) {
        let context = self.context.clone();
        context.run(|| {
            let worker_count = self.workers.len();
            let pool = reth_tasks::WorkerPool::new(worker_count, "trie-verify");
            // Reserve backlog space before dispatch so a slow early root can always finish
            // without competing with later results for space.
            let (jobs_tx, jobs_rx) = crossbeam_channel::bounded(worker_count * 2);
            pool.in_place_scope(|scope| {
                for worker in self.workers.drain(..) {
                    let jobs = jobs_rx.clone();
                    let context = self.context.clone();
                    let span = tracing::Span::current();
                    scope.spawn(move |_| {
                        let _entered = span.enter();
                        context.run(|| worker.run(jobs, &context));
                    });
                }
                drop(jobs_rx);
                // Close announcements before joining workers: account EOF must not depend on
                // workers finishing their findings sends.
                context.run(|| self.produce(jobs_tx, announcements));
            });
            Ok(())
        });
    }

    fn produce(
        &mut self,
        jobs: Sender<(B256, Option<Sender<B256>>)>,
        announcements: Sender<(B256, Receiver<B256>)>,
    ) -> Result<(), StateRootError> {
        for job in self.addresses.by_ref() {
            let job = job?;
            let root = if job.root_required {
                let (sender, receiver) = crossbeam_channel::bounded(1);
                // Announce in scan order before dispatch, regardless of completion order.
                if !self.context.send(&announcements, (job.address, receiver)) {
                    return Ok(());
                }
                Some(sender)
            } else {
                None
            };
            if !self.context.send(&jobs, (job.address, root)) {
                return Ok(());
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
    pub(super) const fn new(
        events: Sender<Result<Output, StateRootError>>,
        cancel: Receiver<()>,
    ) -> Self {
        Self { events, cancel }
    }

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
    pub(super) fn receive<T>(&self, jobs: &Receiver<T>) -> Option<T> {
        crossbeam_channel::select! {
            recv(jobs) -> job => job.ok(),
            recv(self.cancel) -> _ => None,
        }
    }

    /// Returns whether verification was cancelled.
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancel.try_recv() == Err(crossbeam_channel::TryRecvError::Disconnected)
    }

    /// Reports one finding, applying backpressure until it is consumed or verification stops.
    pub(super) fn report_one(&self, output: Output) -> bool {
        self.send(&self.events, Ok(output))
    }

    /// Runs `work`, reporting its error or panic as a verification error.
    pub(super) fn run(&self, work: impl FnOnce() -> Result<(), StateRootError>) {
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
