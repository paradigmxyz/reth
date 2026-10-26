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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{noop::NoopHashedCursor, HashedCursor},
        trie_cursor::noop::NoopStorageTrieCursor,
        Nibbles,
    };
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn full_findings_channel_observes_cancellation() {
        let (events_tx, events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
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
        for pending in [false, true] {
            let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
            let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
            let context = WorkerContext::new(events_tx, cancel_rx);
            let (announcements_tx, announcements) = crossbeam_channel::bounded(1);
            let (_root_tx, root_rx) = crossbeam_channel::bounded(1);
            if pending {
                announcements_tx.send((B256::ZERO, root_rx)).unwrap();
            }
            let mut pipeline =
                StoragePipeline { context, background: None, announcements, next: None };
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let worker = thread::spawn(move || {
                done_tx.send(pipeline.next_root(None)).unwrap();
            });
            drop(cancel_tx);
            assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), None);
            worker.join().unwrap();
        }
    }

    #[test]
    fn roots_are_buffered_without_consuming_findings() {
        let (events_tx, events_rx) = crossbeam_channel::bounded(1);
        let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, announcements) = crossbeam_channel::bounded(2);
        let first = B256::repeat_byte(1);
        let second = B256::repeat_byte(2);
        let (first_tx, first_rx) = crossbeam_channel::bounded(1);
        let (second_tx, second_rx) = crossbeam_channel::bounded(1);
        announcements_tx.send((first, first_rx)).unwrap();
        announcements_tx.send((second, second_rx)).unwrap();
        assert!(context.report_one(Output::Progress(Nibbles::new())));
        second_tx.send(second).unwrap();
        first_tx.send(first).unwrap();
        drop(announcements_tx);
        let mut pipeline = StoragePipeline { context, background: None, announcements, next: None };
        assert_eq!(pipeline.next_root(Some(B256::ZERO)), None);
        // A bound retains just one announcement, leaving subsequent ones in the channel.
        assert_eq!(pipeline.next.as_ref().map(|(address, _)| *address), Some(first));
        assert_eq!(pipeline.announcements.len(), 1);
        assert_eq!(pipeline.next_root(Some(first)), Some((first, first)));
        assert_eq!(pipeline.next_root(Some(first)), None);
        assert_eq!(events_rx.len(), 1);
        assert_eq!(pipeline.next_root(None), Some((second, second)));
        assert_eq!(pipeline.next_root(None), None);
        assert!(pipeline.next.is_none());
    }

    #[test]
    fn root_capacity_wait_observes_cancellation() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, _announcements) = crossbeam_channel::bounded(1);
        let (_root_tx, root_rx) = crossbeam_channel::bounded::<B256>(1);
        announcements_tx.send((B256::ZERO, root_rx.clone())).unwrap();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(context.send(&announcements_tx, (B256::repeat_byte(1), root_rx))).unwrap();
        });
        drop(cancel_tx);
        assert!(!done_rx.recv_timeout(TIMEOUT).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn scan_completion_wakes_empty_root_wait() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, announcements) = crossbeam_channel::bounded(1);
        let mut pipeline = StoragePipeline { context, background: None, announcements, next: None };
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(pipeline.next_root(None)).unwrap();
        });
        drop(announcements_tx);
        assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), None);
        worker.join().unwrap();
    }

    #[test]
    fn pending_first_root_blocks_later_completed_root() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, announcements) = crossbeam_channel::bounded(2);
        let first = B256::repeat_byte(1);
        let second = B256::repeat_byte(2);
        let (first_tx, first_rx) = crossbeam_channel::bounded(1);
        let (second_tx, second_rx) = crossbeam_channel::bounded(1);
        announcements_tx.send((first, first_rx)).unwrap();
        announcements_tx.send((second, second_rx)).unwrap();
        second_tx.send(second).unwrap();
        drop(announcements_tx);
        let mut pipeline = StoragePipeline { context, background: None, announcements, next: None };
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(pipeline.next_root(None)).unwrap();
            assert_eq!(pipeline.next_root(None), Some((second, second)));
        });
        assert_eq!(done_rx.try_recv(), Err(crossbeam_channel::TryRecvError::Empty));
        first_tx.send(first).unwrap();
        assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), Some((first, first)));
        worker.join().unwrap();
    }

    #[test]
    fn consuming_completed_root_wakes_scheduler() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, announcements) = crossbeam_channel::bounded(1);
        let first = B256::repeat_byte(1);
        let second = B256::repeat_byte(2);
        let (first_tx, first_rx) = crossbeam_channel::bounded(1);
        let (_second_tx, second_rx) = crossbeam_channel::bounded(1);
        announcements_tx.send((first, first_rx)).unwrap();
        first_tx.send(first).unwrap();
        let scheduler = context.clone();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(scheduler.send(&announcements_tx, (second, second_rx))).unwrap();
        });
        assert_eq!(done_rx.try_recv(), Err(crossbeam_channel::TryRecvError::Empty));
        let mut pipeline = StoragePipeline { context, background: None, announcements, next: None };
        assert_eq!(pipeline.next_root(None), Some((first, first)));
        assert!(done_rx.recv_timeout(TIMEOUT).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn disconnected_root_preserves_worker_error() {
        let (events_tx, events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, announcements) = crossbeam_channel::bounded(1);
        let (root_tx, root_rx) = crossbeam_channel::bounded(1);
        announcements_tx.send((B256::ZERO, root_rx)).unwrap();
        drop(announcements_tx);
        let reporter = context.clone();
        let mut pipeline = StoragePipeline { context, background: None, announcements, next: None };
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(pipeline.next_root(None)).unwrap();
        });
        reporter.run(|| {
            drop(root_tx);
            Err(DatabaseError::Other("original worker failure".into()).into())
        });
        let error = events_rx.recv_timeout(TIMEOUT).unwrap().unwrap_err();
        assert_eq!(
            error.to_string(),
            StateRootError::from(DatabaseError::Other("original worker failure".into()))
                .to_string()
        );
        assert_eq!(done_rx.try_recv(), Err(crossbeam_channel::TryRecvError::Empty));
        drop(cancel_tx);
        assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), None);
        worker.join().unwrap();
    }

    #[test]
    fn rendezvous_announcement_observes_cancellation() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (announcements_tx, _announcements) = crossbeam_channel::bounded(0);
        let (_root_tx, root_rx) = crossbeam_channel::bounded::<B256>(1);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(context.send(&announcements_tx, (B256::ZERO, root_rx))).unwrap();
        });
        drop(cancel_tx);
        assert!(!done_rx.recv_timeout(TIMEOUT).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn full_job_channel_observes_cancellation() {
        let (events_tx, _events_rx) = crossbeam_channel::bounded(1);
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let (jobs_tx, _jobs_rx) = crossbeam_channel::bounded::<(B256, Option<Sender<B256>>)>(1);
        jobs_tx.send((B256::ZERO, None)).unwrap();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            done_tx.send(context.send(&jobs_tx, (B256::ZERO, None))).unwrap();
        });
        drop(cancel_tx);
        assert!(!done_rx.recv_timeout(TIMEOUT).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn scheduler_bounds_dispatched_roots_with_stalled_first_worker() {
        for limit in [1, 3] {
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let task = thread::spawn(move || {
                let (events_tx, events_rx) = crossbeam_channel::bounded(1);
                let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
                let context = WorkerContext::new(events_tx, cancel_rx);
                let (started_tx, started_rx) = crossbeam_channel::unbounded();
                let (release_tx, release_rx) = crossbeam_channel::bounded(1);
                let (scanned_tx, scanned_rx) = crossbeam_channel::unbounded();
                let jobs = (1..=8).map(move |index| {
                    scanned_tx.send(index).unwrap();
                    Ok(StorageJob { address: B256::repeat_byte(index), root_required: true })
                });
                let workers = (0..4)
                    .map(|_| StorageWorker {
                        trie: NoopStorageTrieCursor::default(),
                        hashed: StalledStorageCursor {
                            inner: NoopHashedCursor::default(),
                            address: B256::ZERO,
                            started: started_tx.clone(),
                            release: release_rx.clone(),
                        },
                        tx: (),
                    })
                    .collect();
                let mut pipeline = StoragePipeline::spawn(jobs, workers, context, limit).unwrap();
                // Retain the first announcement without consuming its root, including when the
                // announcement queue is a rendezvous channel.
                assert_eq!(pipeline.next_root(Some(B256::ZERO)), None);
                let mut started = (0..limit)
                    .map(|_| started_rx.recv_timeout(TIMEOUT).unwrap())
                    .collect::<Vec<_>>();
                started.sort_unstable();
                assert_eq!(
                    started,
                    (1..=limit).map(|index| B256::repeat_byte(index as u8)).collect::<Vec<_>>()
                );
                for index in 1..=limit + 1 {
                    assert_eq!(scanned_rx.recv_timeout(TIMEOUT).unwrap(), index as u8);
                }
                // There are spare workers, but the next scanned root must not be dispatched.
                // Later workers have empty storage and can finish while the earliest is gated.
                assert_eq!(
                    started_rx.recv_timeout(Duration::from_millis(100)),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout)
                );
                assert_eq!(pipeline.announcements.len(), limit - 1);
                release_tx.send_timeout((), TIMEOUT).unwrap();
                assert_eq!(
                    pipeline.next_root(None),
                    Some((B256::repeat_byte(1), alloy_consensus::EMPTY_ROOT_HASH))
                );
                // Receiving the next announcement frees a queue slot and must resume dispatch.
                assert_eq!(pipeline.next_root(Some(B256::ZERO)), None);
                assert_eq!(
                    started_rx.recv_timeout(TIMEOUT).unwrap(),
                    B256::repeat_byte((limit + 1) as u8)
                );
                assert_eq!(
                    pipeline.next_root(None),
                    Some((B256::repeat_byte(2), alloy_consensus::EMPTY_ROOT_HASH))
                );
                drop(cancel_tx);
                drop(pipeline);
                assert!(events_rx.is_empty());
                done_tx.send(()).unwrap();
            });
            done_rx.recv_timeout(TIMEOUT).unwrap();
            task.join().unwrap();
        }
    }

    struct StalledStorageCursor {
        inner: NoopHashedCursor<U256>,
        address: B256,
        started: Sender<B256>,
        release: Receiver<()>,
    }

    impl HashedCursor for StalledStorageCursor {
        type Value = U256;

        fn seek(&mut self, key: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
            self.inner.seek(key)
        }

        fn next(&mut self) -> Result<Option<(B256, U256)>, DatabaseError> {
            self.inner.next()
        }

        fn reset(&mut self) {
            self.inner.reset();
        }
    }

    impl HashedStorageCursor for StalledStorageCursor {
        fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
            self.started.send(self.address).unwrap();
            if self.address == B256::repeat_byte(1) {
                self.release.recv_timeout(TIMEOUT).unwrap();
            }
            self.inner.is_storage_empty()
        }

        fn set_hashed_address(&mut self, address: B256) {
            self.address = address;
            self.inner.set_hashed_address(address);
        }
    }
}
