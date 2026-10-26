//! Storage scheduling, completed-root buffering, shared findings, and background task lifetime.

use super::{Output, StorageJob, StorageWorker};
use crate::{hashed_cursor::HashedStorageCursor, trie_cursor::TrieStorageCursor};
use alloy_primitives::{map::B256Map, B256, U256};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

/// Owns storage scheduling and buffers roots for the account producer.
#[derive(Debug)]
pub(super) struct StoragePipeline {
    roots_rx: Receiver<(B256, B256)>,
    roots: B256Map<B256>,
    context: WorkerContext,
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
        context: WorkerContext,
    ) -> Result<Self, DatabaseError>
    where
        L: Iterator<Item = Result<StorageJob, DatabaseError>> + Send + 'static,
        T: TrieStorageCursor + Send + 'static,
        H: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        assert!(!workers.is_empty(), "verification requires storage workers");
        let (roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let scheduler =
            StorageScheduler { addresses, workers, roots: roots_tx, context: context.clone() };
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-lookahead".into())
            .spawn(move || {
                let _entered = span.enter();
                scheduler.run();
            })
            .map_err(|error| DatabaseError::Custom(Arc::new(error)))?;
        Ok(Self { roots_rx, roots: Default::default(), context, background: Some(background) })
    }

    /// Waits only for roots; the iterator consumes findings independently. Returns `None` on
    /// cancellation. Each non-empty account storage root must be requested exactly once.
    pub(super) fn root(&mut self, address: B256) -> Result<Option<B256>, StateRootError> {
        if let Some(root) = self.roots.remove(&address) {
            self.context.root_window.release();
            return Ok(Some(root));
        }
        loop {
            crossbeam_channel::select! {
                recv(self.context.cancel) -> _ => return Ok(None),
                recv(self.roots_rx) -> root => {
                    let Ok((root_address, root)) = root else {
                        return Err(DatabaseError::Other(format!(
                            "storage root of {address} was never computed"
                        )).into())
                    };
                    if root_address == address {
                        self.context.root_window.release();
                        return Ok(Some(root))
                    }
                    self.roots.insert(root_address, root);
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
    roots: Sender<(B256, B256)>,
    context: WorkerContext,
}

impl<L, T, H, TX> StorageScheduler<L, T, H, TX>
where
    L: Iterator<Item = Result<StorageJob, DatabaseError>>,
    T: TrieStorageCursor + Send,
    H: HashedStorageCursor<Value = U256> + Send,
    TX: Send,
{
    fn run(mut self) {
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
                    let roots = self.roots.clone();
                    let context = self.context.clone();
                    let span = tracing::Span::current();
                    scope.spawn(move |_| {
                        let _entered = span.enter();
                        context.run(|| worker.run(jobs, &roots, &context));
                    });
                }
                drop(jobs_rx);
                context.run(|| self.produce(jobs_tx));
            });
            Ok(())
        });
    }

    fn produce(&mut self, jobs: Sender<StorageJob>) -> Result<(), StateRootError> {
        for job in self.addresses.by_ref() {
            let job = job?;
            if job.root_required && !self.context.root_window.acquire(&self.context.cancel) {
                return Ok(());
            }
            if !self.context.send(&jobs, job) {
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
    pub(super) root_window: Arc<StorageRootWindow>,
}

impl WorkerContext {
    pub(super) fn new(
        events: Sender<Result<Output, StateRootError>>,
        cancel: Receiver<()>,
        storage_root_limit: usize,
    ) -> Self {
        Self { events, cancel, root_window: Arc::new(StorageRootWindow::new(storage_root_limit)) }
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
    pub(super) fn receive(&self, jobs: &Receiver<StorageJob>) -> Option<StorageJob> {
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

/// Bounds dispatched account storage jobs until their roots are consumed. A single wakeup slot
/// avoids allocating one semaphore token per permitted root.
#[derive(Debug)]
pub(super) struct StorageRootWindow {
    available: AtomicUsize,
    wake_tx: Sender<()>,
    wake_rx: Receiver<()>,
}

impl StorageRootWindow {
    fn new(limit: usize) -> Self {
        assert!(limit > 0, "storage root limit must be positive");
        let (wake_tx, wake_rx) = crossbeam_channel::bounded(1);
        Self { available: AtomicUsize::new(limit), wake_tx, wake_rx }
    }

    fn acquire(&self, cancel: &Receiver<()>) -> bool {
        loop {
            if self
                .available
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                return true;
            }
            crossbeam_channel::select! {
                recv(self.wake_rx) -> _ => {},
                recv(cancel) -> _ => return false,
            }
        }
    }

    pub(super) fn release(&self) {
        if self.available.fetch_add(1, Ordering::Relaxed) == 0 {
            match self.wake_tx.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => {}
                // The receiver lives in this same object.
                Err(TrySendError::Disconnected(())) => {
                    unreachable!("root window receiver is alive")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nibbles;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn full_findings_channel_observes_cancellation() {
        let (events_tx, events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx, 1);
        assert!(context.report_one(Output::Progress(Nibbles::new())));
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(context.report_one(Output::Progress(Nibbles::new()))).unwrap();
        });
        assert_eq!(events_rx.len(), 1);
        drop(cancel_tx);
        assert!(!done_rx.recv_timeout(TIMEOUT).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn root_wait_observes_cancellation() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx, 1);
        let (_roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let mut pipeline =
            StoragePipeline { roots_rx, roots: Default::default(), context, background: None };
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(pipeline.root(B256::ZERO).unwrap()).unwrap();
        });
        drop(cancel_tx);
        assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), None);
        worker.join().unwrap();
    }

    #[test]
    fn roots_are_buffered_without_consuming_findings() {
        let (events_tx, events_rx) = crossbeam_channel::bounded(1);
        let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx, 2);
        assert!(context.root_window.acquire(&context.cancel));
        assert!(context.root_window.acquire(&context.cancel));
        assert!(context.report_one(Output::Progress(Nibbles::new())));
        let (roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let first = B256::repeat_byte(1);
        let second = B256::repeat_byte(2);
        roots_tx.send((second, second)).unwrap();
        roots_tx.send((first, first)).unwrap();
        let mut pipeline =
            StoragePipeline { roots_rx, roots: Default::default(), context, background: None };
        assert_eq!(pipeline.root(first).unwrap(), Some(first));
        assert_eq!(pipeline.roots.len(), 1);
        assert_eq!(events_rx.len(), 1);
        assert_eq!(pipeline.root(second).unwrap(), Some(second));
        assert!(pipeline.roots.is_empty());
        assert_eq!(pipeline.context.root_window.available.load(Ordering::Relaxed), 2);
    }
}
