//! Storage scheduling, completed-root buffering, shared findings, and background task lifetime.

use super::{Output, StorageWorker};
use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::TrieStorageCursor,
};
use alloy_primitives::{map::B256Map, B256, U256};
use crossbeam_channel::{Receiver, Sender};
use reth_execution_errors::StateRootError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use std::{
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    thread::{self, JoinHandle},
};

/// Owns background work, buffers completed roots, and merges findings for the account consumer.
#[derive(Debug)]
pub(super) struct StoragePipeline {
    roots_rx: Receiver<(B256, B256)>,
    roots: B256Map<B256>,
    events_tx: Sender<Result<Output, StateRootError>>,
    events_rx: Receiver<Result<Output, StateRootError>>,
    // Disconnecting the sole sender broadcasts cancellation to every worker.
    cancel: Option<Sender<()>>,
    background: Option<JoinHandle<()>>,
}

impl StoragePipeline {
    pub(super) fn spawn<L, T, H, TX>(
        accounts: L,
        workers: Vec<StorageWorker<T, H, TX>>,
    ) -> Result<Self, DatabaseError>
    where
        L: HashedCursor<Value = Account> + Send + 'static,
        T: TrieStorageCursor + Send + 'static,
        H: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        assert!(!workers.is_empty(), "verification requires storage workers");
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let (roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let scheduler = StorageScheduler {
            accounts,
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
            .map_err(|error| DatabaseError::Other(error.to_string()))?;
        Ok(Self {
            roots_rx,
            roots: Default::default(),
            events_tx,
            events_rx,
            cancel: Some(cancel_tx),
            background: Some(background),
        })
    }

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
                recv(self.events_rx) -> event => {
                    if let Ok(event) = event { outputs.push_back(Ok(event?)); }
                }
                recv(self.roots_rx) -> root => {
                    let Ok((account, root)) = root else {
                        self.drain(outputs)?;
                        return Err(DatabaseError::Other("Storage root workers disconnected".into()).into())
                    };
                    if account == address { return Ok(root) }
                    self.roots.insert(account, root);
                }
            }
        }
    }

    pub(super) fn drain(
        &self,
        outputs: &mut VecDeque<Result<Output, StateRootError>>,
    ) -> Result<(), StateRootError> {
        for event in self.events_rx.try_iter() {
            outputs.push_back(Ok(event?));
        }
        Ok(())
    }

    pub(super) fn report(
        &self,
        found: Vec<Output>,
        outputs: &mut VecDeque<Result<Output, StateRootError>>,
    ) -> Result<(), StateRootError> {
        for output in found {
            assert!(self.events_tx.send(Ok(output)).is_ok(), "verifier owns the event receiver");
        }
        self.drain(outputs)
    }

    /// Waits for all scheduled work; queued findings and failures remain available to drain.
    pub(super) fn finish(&mut self) {
        // Joining before cancellation preserves errors from the producer's final cursor read.
        self.join();
        self.cancel.take();
    }

    pub(super) fn stop(&mut self) {
        self.cancel.take();
        self.join();
    }

    fn join(&mut self) {
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

/// Runs lookahead on the coordinator thread and keeps the fixed storage pool supplied with jobs.
struct StorageScheduler<L, T, H, TX> {
    accounts: L,
    workers: Vec<StorageWorker<T, H, TX>>,
    roots: Sender<(B256, B256)>,
    context: WorkerContext,
}

impl<L, T, H, TX> StorageScheduler<L, T, H, TX>
where
    L: HashedCursor<Value = Account>,
    T: TrieStorageCursor + Send,
    H: HashedStorageCursor<Value = U256> + Send,
    TX: Send,
{
    fn run(mut self) {
        let context = self.context.clone();
        context.run(|| {
            let workers = self.workers.len();
            let pool = reth_tasks::WorkerPool::new(workers, "trie-verify");
            // Only queued work is bounded; completed roots never wait for the account thread.
            let (jobs_tx, jobs_rx) = crossbeam_channel::bounded(workers * 2);
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
        let mut next = self.accounts.seek(B256::ZERO)?;
        while let Some((address, _)) = next {
            if !self.context.send(&jobs, address) {
                return Ok(())
            }
            next = self.accounts.next()?;
        }
        Ok(())
    }
}

/// Shared cancellation and reporting policy for the coordinator and storage workers.
#[derive(Clone, Debug)]
pub(super) struct WorkerContext {
    events: Sender<Result<Output, StateRootError>>,
    cancel: Receiver<()>,
}

impl WorkerContext {
    pub(super) fn send<T>(&self, sender: &Sender<T>, value: T) -> bool {
        crossbeam_channel::select! {
            send(sender, value) -> result => result.is_ok(),
            recv(self.cancel) -> _ => false,
        }
    }

    pub(super) fn receive(
        &self,
        jobs: &Receiver<B256>,
    ) -> Result<B256, crossbeam_channel::RecvError> {
        crossbeam_channel::select! {
            recv(jobs) -> job => job,
            recv(self.cancel) -> _ => Err(crossbeam_channel::RecvError),
        }
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancel.try_recv() == Err(crossbeam_channel::TryRecvError::Disconnected)
    }

    pub(super) fn report(&self, found: Vec<Output>) -> bool {
        for output in found {
            if !self.send(&self.events, Ok(output)) {
                return false
            }
        }
        true
    }

    fn run(&self, work: impl FnOnce() -> Result<(), StateRootError>) {
        let result = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
            Err(DatabaseError::Other("Trie verification thread panicked".into()).into())
        });
        if let Err(error) = result {
            self.send(&self.events, Err(error));
        }
    }
}
