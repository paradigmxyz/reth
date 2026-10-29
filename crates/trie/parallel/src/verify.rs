//! Bounded parallel verification of persisted database tries.

use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{map::B256Map, B256};
use crossbeam_channel::{Receiver, RecvTimeoutError, SendTimeoutError, Sender, TryRecvError};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    tables,
    transaction::{DbTx, DupCursorTy},
    DatabaseError,
};
use reth_execution_errors::{StateRootError, StorageRootError};
use reth_provider::{DBProvider, DatabaseProviderFactory, ProviderError};
use reth_tasks::TaskExecutor;
use reth_trie::{
    metrics::TrieRootMetrics,
    trie_cursor::{noop::NoopTrieCursorFactory, TrieCursorFactory, TrieStorageCursor},
    verify::Verifier,
    StateRoot, StateRootProgress, StorageRoot, StorageRootProgress, TrieType,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseHashedStorageCursor, DatabaseStorageTrieCursor,
    DatabaseTrieCursorFactory, TrieTableAdapter,
};
use std::{
    any::Any,
    collections::VecDeque,
    fmt,
    iter::FusedIterator,
    num::NonZeroUsize,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use thiserror::Error;
use tracing::{debug, error, Span};

pub use reth_trie::verify::{NodeDifference, Output, TrieKind};

/// Checks persisted trie tables against hashed state, including orphan storage tries.
///
/// Construction opens two read transactions plus one per effective worker and checks that their
/// snapshot identifiers agree before starting work. A concurrent commit during construction returns
/// [`VerificationError::SnapshotChanged`]; retry when writes are quiescent. No write lock is taken.
/// Long-read safety is disabled on these transactions only. Keep the verifier short-lived on a
/// running node: its snapshot pins old database pages until iteration completes or it is dropped.
///
/// `next()` blocks waiting for findings. Storage findings may interleave with account findings and
/// progress. At most `storage_lookahead` storage roots are queued, running or ready, and findings
/// use a bounded queue. Dropping the iterator cancels and waits for active work. Background
/// failures are returned as errors, never re-raised by `next()` or `Drop`.
///
/// Effective concurrency is the minimum of the worker count, lookahead and executor CPU-pool size.
/// Storage jobs occupy that shared pool and may block on consumer backpressure. Consume and drop
/// the verifier outside the CPU pool: doing either from that pool can prevent queued jobs from
/// starting and deadlock shutdown. On a running node, leave CPU-pool capacity for other work.
///
/// Hashed storage remains the source of truth even without a hashed account row: these tries may
/// yield missing-node findings, while trie-only storage yields extra-node findings. Orphan storage
/// roots never create account leaves. A repair consumer can therefore create storage-trie nodes
/// for orphan hashed storage without changing the account trie or state root.
pub struct DatabaseVerifier {
    findings: Receiver<Output>,
    cancelled: Arc<AtomicBool>,
    coordinator: Option<JoinHandle<Result<(), VerificationError>>>,
}

impl DatabaseVerifier {
    /// Verifies a database using the supplied executor's CPU pool.
    ///
    /// Counts must be nonzero and no greater than [`MAX_STORAGE_WORKER_COUNT`] and
    /// [`MAX_STORAGE_LOOKAHEAD`]. Worker count limits concurrent storage jobs, additionally limited
    /// by the executor's CPU-pool size. Returns an error if snapshots cannot be identified, differ,
    /// a transaction cannot be opened, or the coordinator cannot be started. Read failures
    /// during verification and background panics are yielded by the iterator with the same
    /// error type.
    pub fn new<DB: Database, A: TrieTableAdapter>(
        db: &DB,
        executor: &TaskExecutor,
        storage_worker_count: NonZeroUsize,
        storage_lookahead: NonZeroUsize,
    ) -> Result<Self, VerificationError>
    where
        DB::TX: 'static,
    {
        Self::start::<_, A>(|| Ok(db.tx()?), executor, storage_worker_count, storage_lookahead)
    }

    /// Verifies persisted state through an existing provider factory, without exposing its DB.
    ///
    /// Snapshot, resource, blocking and error behavior is identical to [`Self::new`]. Only
    /// persisted hashed state is checked; unpersisted canonical state held by a provider is not
    /// included.
    pub fn from_provider_factory<F: DatabaseProviderFactory, A: TrieTableAdapter>(
        factory: &F,
        executor: &TaskExecutor,
        storage_worker_count: NonZeroUsize,
        storage_lookahead: NonZeroUsize,
    ) -> Result<Self, VerificationError> {
        Self::start::<_, A>(
            || Ok(factory.database_provider_ro()?.into_tx()),
            executor,
            storage_worker_count,
            storage_lookahead,
        )
    }

    fn start<TX: DbTx + 'static, A: TrieTableAdapter>(
        mut open_tx: impl FnMut() -> Result<TX, VerificationError>,
        executor: &TaskExecutor,
        storage_worker_count: NonZeroUsize,
        storage_lookahead: NonZeroUsize,
    ) -> Result<Self, VerificationError> {
        if storage_worker_count.get() > MAX_STORAGE_WORKER_COUNT {
            return Err(VerificationError::TooManyStorageWorkers {
                requested: storage_worker_count.get(),
                maximum: MAX_STORAGE_WORKER_COUNT,
            });
        }
        if storage_lookahead.get() > MAX_STORAGE_LOOKAHEAD {
            return Err(VerificationError::LookaheadTooLarge {
                requested: storage_lookahead.get(),
                maximum: MAX_STORAGE_LOOKAHEAD,
            });
        }
        let worker_count = storage_worker_count
            .get()
            .min(storage_lookahead.get())
            .min(executor.cpu_pool().current_num_threads());
        let mut account_tx = open_tx()?;
        account_tx.disable_long_read_transaction_safety();
        let snapshot = account_tx.snapshot_id()?.ok_or(VerificationError::SnapshotUnavailable)?;
        let mut open_snapshot = || {
            let mut tx = open_tx()?;
            tx.disable_long_read_transaction_safety();
            let actual = tx.snapshot_id()?.ok_or(VerificationError::SnapshotUnavailable)?;
            if actual != snapshot {
                return Err(VerificationError::SnapshotChanged { expected: snapshot, actual });
            }
            Ok(tx)
        };
        let scan_tx = open_snapshot()?;
        let transactions =
            (0..worker_count).map(|_| open_snapshot()).collect::<Result<Vec<_>, _>>()?;
        let (findings_tx, findings) = crossbeam_channel::bounded(FINDINGS_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let context = VerificationContext {
            findings: findings_tx,
            cancelled: Arc::clone(&cancelled),
            storage_metrics: TrieRootMetrics::new(TrieType::Storage),
        };
        let executor = executor.clone();
        let span = Span::current();
        // The tasks spawning helpers panic on OS spawn failure; preserve the typed I/O error here.
        let coordinator = thread::Builder::new()
            .name("trie-verifier".into())
            .spawn(move || {
                let _entered = span.enter();
                catch_verification(|| {
                    debug!(target: "trie::verify", snapshot, "Starting trie verification");
                    let (completions_tx, completions_rx) = crossbeam_channel::bounded(worker_count);
                    let mut storage = VerificationStorage::<_, A> {
                        addresses: VerificationStorageAddresses::new(&scan_tx)?,
                        workers: transactions
                            .into_iter()
                            .map(VerificationStorageWorker::new)
                            .collect::<Result<_, _>>()?,
                        pending: VecDeque::new(),
                        roots: B256Map::default(),
                        completions_tx,
                        completions_rx,
                        storage_lookahead,
                        executor,
                        context: &context,
                    };
                    storage.dispatch()?;
                    let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(&account_tx);
                    let mut account_verifier = Verifier::new(
                        TrieKind::Account,
                        trie_cursor_factory.account_trie_cursor()?,
                    )?;
                    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&account_tx);
                    let mut state = None;
                    loop {
                        context.check_cancelled()?;
                        let progress = StateRoot::new(
                            NoopTrieCursorFactory::default(),
                            &hashed_cursor_factory,
                        )
                        .with_intermediate_state(state.take())
                        .root_with_progress_with_storage_roots(&mut |hashed_address| {
                            storage.root(hashed_address)
                        })?;
                        let (complete, updates) = match progress {
                            StateRootProgress::Complete(_, _, updates) => (true, updates),
                            StateRootProgress::Progress(next, _, updates) => {
                                state = Some(*next);
                                (false, updates)
                            }
                        };
                        account_verifier.verify_updates(updates.account_nodes, &mut |output| {
                            context.output(output)
                        })?;
                        if complete {
                            account_verifier.finish(&mut |output| context.output(output))?;
                            break;
                        }
                    }
                    // StateRoot visits hashed accounts only. Finish verifying storage without one.
                    while storage.next_root()?.is_some() {}
                    debug!(target: "trie::verify", "Completed trie verification");
                    Ok(())
                })
            })
            .map_err(VerificationError::Spawn)?;
        Ok(Self { findings, cancelled, coordinator: Some(coordinator) })
    }

    fn join(&mut self) -> Result<(), VerificationError> {
        if let Some(coordinator) = self.coordinator.take() {
            return coordinator
                .join()
                .map_err(|payload| VerificationError::Panicked(Mutex::new(payload)))?;
        }
        Ok(())
    }
}

impl Iterator for DatabaseVerifier {
    type Item = Result<Output, VerificationError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.coordinator.as_ref()?;
        match self.findings.recv() {
            Ok(output) => Some(Ok(output)),
            // EOF is confirmed by the coordinator's result, never inferred from a missing root.
            Err(_) => self.join().err().map(Err),
        }
    }
}

impl FusedIterator for DatabaseVerifier {}

impl Drop for DatabaseVerifier {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Err(error) = self.join() &&
            !matches!(error, VerificationError::Cancelled)
        {
            error!(target: "trie::verify", ?error, "Trie verification failed during shutdown");
        }
    }
}

impl fmt::Debug for DatabaseVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseVerifier")
            .field("running", &self.coordinator.is_some())
            .finish_non_exhaustive()
    }
}

/// Errors from construction and iteration of [`DatabaseVerifier`].
#[derive(Debug, Error)]
pub enum VerificationError {
    /// A database operation failed, regardless of which trie was being verified.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Opening a provider failed for a reason other than a database error.
    #[error(transparent)]
    Provider(ProviderError),
    /// The requested number of storage jobs exceeds the supported resource limit.
    #[error("storage worker count {requested} exceeds the maximum {maximum}")]
    TooManyStorageWorkers {
        /// Requested count.
        requested: usize,
        /// Largest supported count.
        maximum: usize,
    },
    /// The requested root backlog exceeds the supported resource limit.
    #[error("storage lookahead {requested} exceeds the maximum {maximum}")]
    LookaheadTooLarge {
        /// Requested backlog.
        requested: usize,
        /// Largest supported backlog.
        maximum: usize,
    },
    /// The database backend cannot identify read snapshots.
    #[error("database backend does not identify read snapshots")]
    SnapshotUnavailable,
    /// A commit occurred while the verifier opened its read snapshots.
    #[error("database changed during verifier construction (snapshot {expected} became {actual})")]
    SnapshotChanged {
        /// Snapshot of the first transaction.
        expected: u64,
        /// Snapshot of a later transaction.
        actual: u64,
    },
    /// Starting the coordinator failed.
    #[error("could not start trie verification")]
    Spawn(#[source] std::io::Error),
    /// A background calculation panicked. The original payload is retained for inspection.
    #[error("trie verification panicked: {}", panic_message(.0))]
    Panicked(Mutex<Box<dyn Any + Send>>),
    /// Verification was cancelled by dropping its consumer.
    #[error("trie verification cancelled")]
    Cancelled,
}

impl From<StorageRootError> for VerificationError {
    fn from(error: StorageRootError) -> Self {
        Self::Database(error.into())
    }
}

impl From<StateRootError> for VerificationError {
    fn from(error: StateRootError) -> Self {
        match error {
            StateRootError::Database(error) => Self::Database(error),
            StateRootError::StorageRootError(error) => error.into(),
            StateRootError::PrefixSetLoadError(error) => error.into(),
        }
    }
}

impl From<ProviderError> for VerificationError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::Database(error) => Self::Database(error),
            error => Self::Provider(error),
        }
    }
}

/// Consumer-owned cancellation and bounded output delivery, shared by active calculations.
#[derive(Clone)]
struct VerificationContext {
    findings: Sender<Output>,
    cancelled: Arc<AtomicBool>,
    storage_metrics: TrieRootMetrics,
}

impl VerificationContext {
    fn check_cancelled(&self) -> Result<(), VerificationError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(VerificationError::Cancelled);
        }
        Ok(())
    }

    fn output(&self, mut output: Output) -> Result<(), VerificationError> {
        loop {
            self.check_cancelled()?;
            match self.findings.send_timeout(output, CANCELLATION_POLL_INTERVAL) {
                Ok(()) => return Ok(()),
                Err(SendTimeoutError::Timeout(pending)) => output = pending,
                // Only the consuming verifier owns a receiver; its disappearance is cancellation.
                Err(SendTimeoutError::Disconnected(_)) => return Err(VerificationError::Cancelled),
            }
        }
    }
}

/// Supplies ordered canonical roots while dispatching independent jobs to the injected pool.
struct VerificationStorage<'a, TX: DbTx + 'static, A: TrieTableAdapter> {
    addresses: VerificationStorageAddresses<TX, A>,
    workers: Vec<VerificationStorageWorker<TX, A>>,
    pending: VecDeque<B256>,
    roots: B256Map<Result<B256, VerificationError>>,
    completions_tx: Sender<VerificationCompletion<TX, A>>,
    completions_rx: Receiver<VerificationCompletion<TX, A>>,
    storage_lookahead: NonZeroUsize,
    executor: TaskExecutor,
    context: &'a VerificationContext,
}

impl<TX: DbTx + 'static, A: TrieTableAdapter> VerificationStorage<'_, TX, A> {
    fn root(&mut self, hashed_address: B256) -> Result<B256, VerificationError> {
        loop {
            self.dispatch()?;
            if self.pending.front().is_none_or(|next| *next > hashed_address) {
                return Ok(EMPTY_ROOT_HASH);
            }
            if let Some((next_address, root)) = self.next_root()? &&
                next_address == hashed_address
            {
                return Ok(root);
            }
            // Roots preceding this account belong to orphan storage and are discarded.
        }
    }

    /// Returns None only after all dispatched roots and the fallible address scan reach EOF.
    fn next_root(&mut self) -> Result<Option<(B256, B256)>, VerificationError> {
        loop {
            self.dispatch()?;
            let Some(&hashed_address) = self.pending.front() else { return Ok(None) };
            if let Some(result) = self.roots.remove(&hashed_address) {
                self.pending.pop_front();
                return Ok(Some((hashed_address, result?)));
            }
            match self.completions_rx.recv_timeout(CANCELLATION_POLL_INTERVAL) {
                Ok(completion) => self.complete(completion),
                Err(RecvTimeoutError::Timeout) => self.context.check_cancelled()?,
                Err(RecvTimeoutError::Disconnected) => {
                    unreachable!("the coordinator retains a completion sender")
                }
            }
        }
    }

    fn complete(&mut self, completion: VerificationCompletion<TX, A>) {
        self.workers.push(completion.worker);
        self.roots.insert(completion.hashed_address, completion.result);
    }

    fn dispatch(&mut self) -> Result<(), VerificationError> {
        self.context.check_cancelled()?;
        loop {
            match self.completions_rx.try_recv() {
                Ok(completion) => self.complete(completion),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    unreachable!("the coordinator retains a completion sender")
                }
            }
        }
        while self.pending.len() < self.storage_lookahead.get() {
            let Some(mut worker) = self.workers.pop() else { break };
            let Some(hashed_address) = self.addresses.next_address()? else {
                self.workers.push(worker);
                break;
            };
            self.pending.push_back(hashed_address);
            let context = self.context.clone();
            let completions = self.completions_tx.clone();
            let span = Span::current();
            self.executor.cpu_pool().spawn(move || {
                let _entered = span.enter();
                let result =
                    catch_verification(|| verify_storage(&mut worker, hashed_address, &context));
                // There can be at most one completion per worker, so this queue cannot fill.
                // A disconnected receiver means the owning verifier has already been dropped.
                if completions
                    .send(VerificationCompletion { worker, hashed_address, result })
                    .is_err()
                {
                    debug!(target: "trie::verify", "Trie verification completion receiver dropped");
                }
            });
        }
        Ok(())
    }
}

impl<TX: DbTx + 'static, A: TrieTableAdapter> Drop for VerificationStorage<'_, TX, A> {
    fn drop(&mut self) {
        self.context.cancelled.store(true, Ordering::Relaxed);
        // Tasks own their worker until sending completion, even on a caught panic. Drain
        // before releasing the scan and account snapshots; cancellation releases findings sends.
        while self.pending.len() > self.roots.len() {
            match self.completions_rx.recv_timeout(CANCELLATION_POLL_INTERVAL) {
                Ok(completion) => self.complete(completion),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    unreachable!("the coordinator retains a completion sender");
                }
            }
        }
        for result in self.roots.values() {
            if let Err(error) = result &&
                !matches!(error, VerificationError::Cancelled)
            {
                error!(target: "trie::verify", ?error, "Additional storage verification failure");
            }
        }
    }
}

/// Reuses storage cursors across jobs while retaining their snapshot transaction.
struct VerificationStorageWorker<TX: DbTx, A: TrieTableAdapter> {
    storage_trie_cursor: DatabaseStorageTrieCursor<DupCursorTy<TX, A::StorageTrieTable>, A>,
    hashed_storage_cursor: DatabaseHashedStorageCursor<DupCursorTy<TX, tables::HashedStorages>>,
    // Backs both cursors, so it must be dropped after them.
    tx: TX,
}

impl<TX: DbTx, A: TrieTableAdapter> VerificationStorageWorker<TX, A> {
    fn new(tx: TX) -> Result<Self, DatabaseError> {
        Ok(Self {
            storage_trie_cursor: DatabaseStorageTrieCursor::new(
                tx.cursor_dup_read::<A::StorageTrieTable>()?,
                B256::ZERO,
            ),
            hashed_storage_cursor: DatabaseHashedStorageCursor::new(
                tx.cursor_dup_read::<tables::HashedStorages>()?,
                B256::ZERO,
            ),
            tx,
        })
    }
}

struct VerificationCompletion<TX: DbTx, A: TrieTableAdapter> {
    worker: VerificationStorageWorker<TX, A>,
    hashed_address: B256,
    result: Result<B256, VerificationError>,
}

fn verify_storage<TX: DbTx, A: TrieTableAdapter>(
    worker: &mut VerificationStorageWorker<TX, A>,
    hashed_address: B256,
    context: &VerificationContext,
) -> Result<B256, VerificationError> {
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&worker.tx);
    let mut noop_cursor = NoopTrieCursorFactory::default().storage_trie_cursor(B256::ZERO)?;
    context.check_cancelled()?;
    worker.storage_trie_cursor.set_hashed_address(hashed_address);
    let mut verifier =
        Verifier::new(TrieKind::Storage { hashed_address }, &mut worker.storage_trie_cursor)?;
    let mut state = None;
    loop {
        context.check_cancelled()?;
        let progress = StorageRoot::new_hashed(
            NoopTrieCursorFactory::default(),
            &hashed_cursor_factory,
            hashed_address,
            Default::default(),
            context.storage_metrics.clone(),
        )
        .with_intermediate_state(state.take())
        .root_with_progress_with_cursors(&mut noop_cursor, &mut worker.hashed_storage_cursor)?;
        let (root, updates) = match progress {
            StorageRootProgress::Complete(root, _, updates) => (Some(root), updates),
            StorageRootProgress::Progress(next, _, updates) => {
                state = Some(*next);
                (None, updates)
            }
        };
        verifier.verify_updates(updates.storage_nodes, &mut |output| context.output(output))?;
        if let Some(root) = root {
            verifier.finish(&mut |output| context.output(output))?;
            return Ok(root);
        }
    }
}

/// Ordered union of hashed storage and stored storage-trie addresses, including orphan tries.
struct VerificationStorageAddresses<TX: DbTx, A: TrieTableAdapter> {
    hashed_storage_cursor: DupCursorTy<TX, tables::HashedStorages>,
    storage_trie_cursor: DupCursorTy<TX, A::StorageTrieTable>,
    next_hashed_address: Option<B256>,
    next_trie_address: Option<B256>,
}

impl<TX: DbTx, A: TrieTableAdapter> VerificationStorageAddresses<TX, A> {
    fn new(tx: &TX) -> Result<Self, DatabaseError> {
        let mut hashed_storage_cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
        let mut storage_trie_cursor = tx.cursor_dup_read::<A::StorageTrieTable>()?;
        let next_hashed_address =
            hashed_storage_cursor.first()?.map(|(hashed_address, _)| hashed_address);
        let next_trie_address =
            storage_trie_cursor.first()?.map(|(hashed_address, _)| hashed_address);
        Ok(Self {
            hashed_storage_cursor,
            storage_trie_cursor,
            next_hashed_address,
            next_trie_address,
        })
    }

    fn next_address(&mut self) -> Result<Option<B256>, DatabaseError> {
        let hashed_address = match (self.next_hashed_address, self.next_trie_address) {
            (Some(hashed), Some(trie)) => hashed.min(trie),
            (Some(hashed_address), None) | (None, Some(hashed_address)) => hashed_address,
            (None, None) => return Ok(None),
        };
        if self.next_hashed_address == Some(hashed_address) {
            self.next_hashed_address =
                self.hashed_storage_cursor.next_no_dup()?.map(|(hashed_address, _)| hashed_address);
        }
        if self.next_trie_address == Some(hashed_address) {
            self.next_trie_address =
                self.storage_trie_cursor.next_no_dup()?.map(|(hashed_address, _)| hashed_address);
        }
        Ok(Some(hashed_address))
    }
}

fn catch_verification<T>(
    work: impl FnOnce() -> Result<T, VerificationError>,
) -> Result<T, VerificationError> {
    catch_unwind(AssertUnwindSafe(work))
        .map_err(|payload| VerificationError::Panicked(Mutex::new(payload)))?
}

const FINDINGS_CAPACITY: usize = 1024;

const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Largest supported number of concurrent storage jobs and their read transactions.
pub const MAX_STORAGE_WORKER_COUNT: usize = 4096;

/// Largest supported root backlog. Allocations grow with actual results, not with this limit.
///
/// This permits the one-million-root tuning range while capping retained address/root metadata.
pub const MAX_STORAGE_LOOKAHEAD: usize = 1_048_576;

fn panic_message(payload: &Mutex<Box<dyn Any + Send>>) -> String {
    let Ok(payload) = payload.lock() else { return "panic payload lock poisoned".into() };
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else {
        "non-string panic payload".into()
    }
}

/// Default bound on queued, running and completed storage roots.
pub const DEFAULT_STORAGE_LOOKAHEAD: NonZeroUsize = NonZeroUsize::new(1024).unwrap();
