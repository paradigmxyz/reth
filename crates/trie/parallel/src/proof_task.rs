//! Parallel proof computation using worker pools with dedicated database transactions.
//!
//!
//! # Architecture
//!
//! - **Worker Pools**: Pre-spawned workers with dedicated database transactions
//!   - Storage pool: Handles storage proofs, one account's storage trie per job
//!   - Account pool: Handles account proofs, calculating storage roots inline
//! - **Automatic Shutdown**: Workers terminate gracefully when the handle is dropped
//!
//! # Message Flow
//!
//! 1. [`ProofWorkerHandle::dispatch`] splits proof targets into storage and account jobs and queues
//!    each job for the matching worker pool.
//! 2. A worker receives a job, runs the proof, and sends one [`ProofWorkerResult`] to the handle's
//!    result channel.
//! 3. The owner of the handle receives the results through [`ProofWorkerHandle::results`].
//!
//! ```text
//! ProofWorkerHandle::dispatch -> Storage/Account Worker
//!                                          |
//!                                          v
//! ProofWorkerHandle::results <-- ProofWorkerResult
//! ```

mod value_encoder;

use self::value_encoder::AccountValueEncoder;
use crate::error::StateRootTaskError;
use alloy_primitives::{B256, U256};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_execution_errors::StateProofError;
use reth_primitives_traits::FastInstant as Instant;
use reth_provider::{DatabaseProviderROFactory, ProviderError, ProviderResult};
use reth_tasks::{Runtime, WorkerPool};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    proof_v2,
    trie_cursor::{TrieCursorFactory, TrieStorageCursor},
    DecodedMultiProofV2, MultiProofTargetsV2, ProofV2Target,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, debug_span, error, instrument, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::{ProofTaskCursorMetricsCache, ProofWorkerMetrics};

/// Type alias for the V2 account proof calculator with worker cursors.
type V2AccountProofCalculator<'a, Provider> = proof_v2::ProofCalculator<
    WorkerTrieCursor<'a, <Provider as TrieCursorFactory>::AccountTrieCursor<'a>>,
    WorkerHashedCursor<'a, <Provider as HashedCursorFactory>::AccountCursor<'a>>,
    AccountValueEncoder<
        WorkerTrieCursor<'a, <Provider as TrieCursorFactory>::StorageTrieCursor<'a>>,
        WorkerHashedCursor<'a, <Provider as HashedCursorFactory>::StorageCursor<'a>>,
    >,
>;

/// Type alias for the V2 storage proof calculator with worker cursors.
type V2StorageProofCalculator<'a, Provider> = proof_v2::StorageProofCalculator<
    WorkerTrieCursor<'a, <Provider as TrieCursorFactory>::StorageTrieCursor<'a>>,
    WorkerHashedCursor<'a, <Provider as HashedCursorFactory>::StorageCursor<'a>>,
>;

/// Trie cursor used by proof workers, instrumented to record cursor metrics.
#[cfg(feature = "metrics")]
type WorkerTrieCursor<'a, C> = reth_trie::trie_cursor::InstrumentedTrieCursor<'a, C>;

/// Trie cursor used by proof workers. Without the `metrics` feature the cursor is used as is, so
/// cursor operations are not timed or counted.
#[cfg(not(feature = "metrics"))]
type WorkerTrieCursor<'a, C> = C;

/// Hashed cursor used by proof workers, instrumented to record cursor metrics.
#[cfg(feature = "metrics")]
type WorkerHashedCursor<'a, C> = reth_trie::hashed_cursor::InstrumentedHashedCursor<'a, C>;

/// Hashed cursor used by proof workers. Without the `metrics` feature the cursor is used as is, so
/// cursor operations are not timed or counted.
#[cfg(not(feature = "metrics"))]
type WorkerHashedCursor<'a, C> = C;

/// Tracks worker availability counts.
///
/// It uses cacheline-aligned flags to avoid core-to-core chatter.
#[derive(Debug)]
struct AvailabilitySheet {
    /// One flag per worker, each on its own cacheline. Workers store `true` when idle,
    /// `false` when busy. Only the owning worker writes; the dispatcher only reads.
    flags: Vec<crossbeam_utils::CachePadded<AtomicBool>>,
}

impl AvailabilitySheet {
    /// Creates a new sheet with `count` workers, all initially marked as busy.
    fn new(count: usize) -> Self {
        let flags =
            (0..count).map(|_| crossbeam_utils::CachePadded::new(AtomicBool::new(false))).collect();
        Self { flags }
    }

    /// Returns `true` if more than one worker is currently idle.
    ///
    /// Note, that this is somewhat racy since a flag that was just saying `idle` and we counted it
    /// as such might turn into `busy` right away.
    fn has_multiple_idle(&self) -> bool {
        let mut idle = 0u32;
        for flag in &self.flags {
            if flag.load(Ordering::Relaxed) {
                idle += 1;
                if idle > 1 {
                    return true;
                }
            }
        }
        false
    }

    /// Marks the given worker as idle.
    fn mark_idle(&self, worker_id: usize) {
        self.flags[worker_id].store(true, Ordering::Relaxed);
    }

    /// Marks the given worker as busy.
    fn mark_busy(&self, worker_id: usize) {
        self.flags[worker_id].store(false, Ordering::Relaxed);
    }
}

/// Owns the storage and account proof worker pools and the channel their results arrive on.
///
/// Every job queued by [`Self::dispatch`] sends exactly one [`ProofWorkerResult`] to
/// [`Self::results`]. A worker that fails to start sends an error that belongs to no job instead,
/// and jobs left queued when every worker of a pool has failed get no result. Workers shut down
/// once the handle is dropped.
#[derive(Debug)]
pub struct ProofWorkerHandle {
    /// Sender shared with the workers, kept so that [`Self::results`] never disconnects while the
    /// handle is alive.
    _result_tx: CrossbeamSender<ProofWorkerResult>,
    /// Results sent by both worker pools.
    result_rx: CrossbeamReceiver<ProofWorkerResult>,
    /// Direct sender to storage worker pool
    storage_work_tx: CrossbeamSender<StorageProofJob>,
    /// Direct sender to account worker pool
    account_work_tx: CrossbeamSender<AccountProofJob>,
    /// Per-worker availability flags for storage workers. Used to determine whether to chunk
    /// multiproofs.
    storage_availability: Arc<AvailabilitySheet>,
    /// Per-worker availability flags for account workers. Used to determine whether to chunk
    /// multiproofs.
    account_availability: Arc<AvailabilitySheet>,
    /// Total number of storage workers spawned
    storage_worker_count: usize,
    /// Total number of account workers spawned
    account_worker_count: usize,
}

impl ProofWorkerHandle {
    /// Spawns storage and account worker pools with dedicated database transactions.
    ///
    /// Returns a handle for submitting proof tasks to the worker pools.
    /// Workers run until the handle is dropped.
    ///
    /// # Parameters
    /// - `runtime`: The centralized runtime used to spawn blocking worker tasks
    /// - `task_ctx`: Shared context with database view and prefix sets
    /// - `worker_counts`: How many workers of each pool to spawn for this block, with a minimum of
    ///   one worker of each kind
    #[instrument(
        name = "ProofWorkerHandle::new",
        level = "debug",
        target = "trie::proof_task",
        skip_all
    )]
    pub fn new<Factory>(
        runtime: &Runtime,
        task_ctx: ProofTaskCtx<Factory>,
        worker_counts: ProofWorkerCounts,
    ) -> Self
    where
        Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let (result_tx, result_rx) = unbounded();
        let (storage_work_tx, storage_work_rx) = unbounded();
        let (account_work_tx, account_work_rx) = unbounded();

        let storage_worker_count =
            worker_counts.storage.max(1).min(runtime.proof_storage_worker_pool().num_threads());
        let account_worker_count =
            worker_counts.account.max(1).min(runtime.proof_account_worker_pool().num_threads());

        let storage_availability = Arc::new(AvailabilitySheet::new(storage_worker_count));
        let account_availability = Arc::new(AvailabilitySheet::new(account_worker_count));

        debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            "Spawning proof worker pools"
        );

        // Registered once per process and shared by all workers of both pools, which record into
        // the same metric series.
        #[cfg(feature = "metrics")]
        let metrics = ProofWorkerMetrics::shared();

        spawn_workers(
            runtime,
            ProofWorkerKind::Storage,
            storage_worker_count,
            ProofWorker {
                task_ctx: task_ctx.clone(),
                work_rx: storage_work_rx,
                result_tx: result_tx.clone(),
                availability: storage_availability.clone(),
                #[cfg(feature = "metrics")]
                metrics: metrics.clone(),
            },
            ProofWorker::<_, StorageProofJob>::run,
        );

        spawn_workers(
            runtime,
            ProofWorkerKind::Account,
            account_worker_count,
            ProofWorker {
                task_ctx,
                work_rx: account_work_rx,
                result_tx: result_tx.clone(),
                availability: account_availability.clone(),
                #[cfg(feature = "metrics")]
                metrics,
            },
            ProofWorker::<_, AccountProofJob>::run,
        );

        Self {
            _result_tx: result_tx,
            result_rx,
            storage_work_tx,
            account_work_tx,
            storage_availability,
            account_availability,
            storage_worker_count,
            account_worker_count,
        }
    }

    /// Returns `true` if more than one storage worker is currently idle.
    pub fn has_multiple_idle_storage_workers(&self) -> bool {
        self.storage_availability.has_multiple_idle()
    }

    /// Returns `true` if more than one account worker is currently idle.
    pub fn has_multiple_idle_account_workers(&self) -> bool {
        self.account_availability.has_multiple_idle()
    }

    /// Returns the total number of storage workers in the pool.
    pub const fn total_storage_workers(&self) -> usize {
        self.storage_worker_count
    }

    /// Returns the total number of account workers in the pool.
    pub const fn total_account_workers(&self) -> usize {
        self.account_worker_count
    }

    /// Returns the receiver of job results.
    ///
    /// Results arrive in completion order. The receiver never disconnects while the handle is
    /// alive.
    pub const fn results(&self) -> &CrossbeamReceiver<ProofWorkerResult> {
        &self.result_rx
    }

    /// Queues the proofs for `targets` and returns the number of jobs queued.
    ///
    /// The storage targets of each account become one storage job, and all account targets
    /// together become one account job. Each job sends exactly one [`ProofWorkerResult`] to
    /// [`Self::results`]. A storage job without slot targets proves only the account's storage
    /// root node.
    ///
    /// Fails once a worker pool has shut down, which only happens after its workers reported start
    /// failures on [`Self::results`].
    pub fn dispatch(&self, targets: MultiProofTargetsV2) -> Result<usize, ProofWorkersUnavailable> {
        let MultiProofTargetsV2 { account_targets, storage_targets } = targets;
        let dispatched_at = Instant::now();
        let mut jobs = 0;

        for (hashed_address, targets) in storage_targets {
            let job = StorageProofJob { hashed_address, targets, dispatched_at };
            self.storage_work_tx.send(job).map_err(|_| ProofWorkersUnavailable)?;
            jobs += 1;
        }

        if !account_targets.is_empty() {
            let job = AccountProofJob { targets: account_targets, dispatched_at };
            self.account_work_tx.send(job).map_err(|_| ProofWorkersUnavailable)?;
            jobs += 1;
        }

        Ok(jobs)
    }
}

/// A message received from [`ProofWorkerHandle::results`]: the outcome of one queued job, or a
/// worker's failure to start.
///
/// A storage job yields the storage proof of a single account and no account proofs. An account
/// job yields account proofs and no storage proofs. A failed job does not stop its worker.
pub type ProofWorkerResult = Result<DecodedMultiProofV2, StateRootTaskError>;

/// Error returned by [`ProofWorkerHandle::dispatch`] when a worker pool has shut down.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("proof workers unavailable")]
pub struct ProofWorkersUnavailable;

/// How many workers [`ProofWorkerHandle::new`] spawns for a block.
///
/// Worker instances are recreated for each block on persistent thread pools. Each worker opens a
/// database transaction and trie cursors, so the caller sizes the counts per block. Counts are
/// clamped to at least one worker of each kind and capped by that kind's pool capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofWorkerCounts {
    /// Number of storage proof workers to spawn.
    pub storage: usize,
    /// Number of account proof workers to spawn.
    pub account: usize,
}

impl ProofWorkerCounts {
    /// Creates counts for the given number of storage and account workers.
    pub const fn new(storage: usize, account: usize) -> Self {
        Self { storage, account }
    }

    /// Creates counts that use every thread of both pools.
    pub fn full(runtime: &Runtime) -> Self {
        Self::new(
            runtime.proof_storage_worker_pool().num_threads(),
            runtime.proof_account_worker_pool().num_threads(),
        )
    }
}

/// Data used for initializing cursor factories that is shared across all proof worker instances.
#[derive(Clone, Debug)]
pub struct ProofTaskCtx<Factory> {
    /// The factory for creating state providers.
    factory: Factory,
    /// Maximum random jitter to apply before each proof computation (trie-debug only).
    #[cfg(feature = "trie-debug")]
    proof_jitter: Option<Duration>,
}

impl<Factory> ProofTaskCtx<Factory> {
    /// Creates a new [`ProofTaskCtx`] with the given factory.
    pub const fn new(factory: Factory) -> Self {
        Self {
            factory,
            #[cfg(feature = "trie-debug")]
            proof_jitter: None,
        }
    }

    /// Sets the maximum proof jitter duration (trie-debug only).
    #[cfg(feature = "trie-debug")]
    pub const fn with_proof_jitter(mut self, jitter: Option<Duration>) -> Self {
        self.proof_jitter = jitter;
        self
    }
}

/// The worker pool a proof worker belongs to.
#[derive(Clone, Copy, Debug)]
enum ProofWorkerKind {
    Storage,
    Account,
}

impl ProofWorkerKind {
    /// Returns the runtime pool the workers of this kind run on.
    fn pool(self, runtime: &Runtime) -> &WorkerPool {
        match self {
            Self::Storage => runtime.proof_storage_worker_pool(),
            Self::Account => runtime.proof_account_worker_pool(),
        }
    }

    /// Returns the name of the pool used in diagnostics.
    const fn name(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Account => "account",
        }
    }

    /// Returns the name of the blocking task that hosts the pool's workers.
    const fn task_name(self) -> &'static str {
        match self {
            Self::Storage => "storage-workers",
            Self::Account => "account-workers",
        }
    }

    /// Returns the span for the worker with the given ID.
    fn span(self, parent: &tracing::Span, worker_id: usize) -> tracing::Span {
        match self {
            Self::Storage => {
                debug_span!(target: "trie::proof_task", parent: parent, "storage_worker", ?worker_id)
            }
            Self::Account => {
                debug_span!(target: "trie::proof_task", parent: parent, "account_worker", ?worker_id)
            }
        }
    }
}

/// Runs `run` as `worker_count` workers of the given kind on a blocking task.
///
/// A worker that fails to start sends a [`ProofWorkerError`] to the result channel.
fn spawn_workers<Factory, J>(
    runtime: &Runtime,
    kind: ProofWorkerKind,
    worker_count: usize,
    worker: ProofWorker<Factory, J>,
    run: fn(&ProofWorker<Factory, J>, usize) -> ProviderResult<()>,
) where
    Factory: Send + Sync + 'static,
    J: Send + 'static,
{
    let rt = runtime.clone();
    let parent_span = tracing::Span::current();
    runtime.spawn_blocking_named(kind.task_name(), move || {
        let next_worker_id = AtomicUsize::new(0);
        kind.pool(&rt).broadcast(worker_count, |_| {
            let worker_id = next_worker_id.fetch_add(1, Ordering::Relaxed);
            let _guard = kind.span(&parent_span, worker_id).entered();

            if let Err(error) = run(&worker, worker_id) {
                error!(
                    target: "trie::proof_task",
                    kind = kind.name(),
                    worker_id,
                    ?error,
                    "Proof worker failed"
                );
                let error = StateRootTaskError::Other(Box::new(ProofWorkerError {
                    kind,
                    worker_id,
                    source: error,
                }));
                // The send fails only once the handle is dropped, when nobody waits for results.
                let _ = worker.result_tx.send(Err(error));
            }
        });
    });
}

/// State shared by the workers of one pool, each holding its own database transaction.
struct ProofWorker<Factory, J> {
    /// Shared task context with database factory and prefix sets
    task_ctx: ProofTaskCtx<Factory>,
    /// Channel for receiving jobs
    work_rx: CrossbeamReceiver<J>,
    /// Sender for job results and worker failures
    result_tx: CrossbeamSender<ProofWorkerResult>,
    /// Per-worker availability flags
    availability: Arc<AvailabilitySheet>,
    /// Metrics shared by all workers of the handle.
    #[cfg(feature = "metrics")]
    metrics: Arc<ProofWorkerMetrics>,
}

impl<Factory, J> ProofWorker<Factory, J> {
    /// Runs `process` for every job until the handle is dropped, advertising the worker as busy
    /// while a job runs. Returns the total time spent waiting for jobs.
    fn process_jobs(&self, worker_id: usize, mut process: impl FnMut(J)) -> Duration {
        // Count this worker as available only after successful initialization.
        self.availability.mark_idle(worker_id);

        let mut total_idle_time = Duration::ZERO;
        let mut idle_start = Instant::now();

        while let Ok(job) = self.work_rx.recv() {
            total_idle_time += idle_start.elapsed();
            self.availability.mark_busy(worker_id);

            #[cfg(feature = "trie-debug")]
            if let Some(max_jitter) = self.task_ctx.proof_jitter {
                let jitter =
                    Duration::from_nanos(rand::random_range(0..=max_jitter.as_nanos() as u64));
                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    jitter_us = jitter.as_micros(),
                    "Proof worker applying proof jitter"
                );
                std::thread::sleep(jitter);
            }

            process(job);

            self.availability.mark_idle(worker_id);
            idle_start = Instant::now();
        }

        total_idle_time
    }

    /// Sends a job result. The send fails only once the handle is dropped, when the result is no
    /// longer needed.
    fn send_result(&self, worker_id: usize, result: ProofWorkerResult) {
        if self.result_tx.send(result).is_err() {
            trace!(
                target: "trie::proof_task",
                worker_id,
                "Proof result receiver dropped, discarding result"
            );
        }
    }
}

impl<Factory> ProofWorker<Factory, StorageProofJob>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    /// Opens a database transaction and serves storage proof jobs until the handle is dropped.
    fn run(&self, worker_id: usize) -> ProviderResult<()> {
        let provider = self.task_ctx.factory.database_provider_ro()?;

        trace!(target: "trie::proof_task", worker_id, "Storage worker started");

        #[cfg(feature = "metrics")]
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();
        let trie_cursor = provider.storage_trie_cursor(B256::ZERO)?;
        let hashed_cursor = provider.hashed_storage_cursor(B256::ZERO)?;
        #[cfg(feature = "metrics")]
        let trie_cursor = reth_trie::trie_cursor::InstrumentedTrieCursor::new(
            trie_cursor,
            &mut cursor_metrics_cache.storage_trie_cursor,
        );
        #[cfg(feature = "metrics")]
        let hashed_cursor = reth_trie::hashed_cursor::InstrumentedHashedCursor::new(
            hashed_cursor,
            &mut cursor_metrics_cache.storage_hashed_cursor,
        );
        let mut calculator =
            proof_v2::StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        let mut storage_proofs_processed = 0u64;
        let total_idle_time = self.process_jobs(worker_id, |job| {
            let result = Self::storage_proof(&mut calculator, job, worker_id);
            self.send_result(worker_id, result);
            storage_proofs_processed += 1;
        });

        // Drop calculator to release mutable borrows on cursor_metrics_cache.
        drop(calculator);

        trace!(
            target: "trie::proof_task",
            worker_id,
            storage_proofs_processed,
            total_idle_time_us = total_idle_time.as_micros(),
            "Storage worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.idle.record_storage_worker_idle_time(total_idle_time);
            self.metrics.cursor.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Calculates the storage proof for a job.
    fn storage_proof<TC, HC>(
        calculator: &mut proof_v2::StorageProofCalculator<TC, HC>,
        job: StorageProofJob,
        worker_id: usize,
    ) -> ProofWorkerResult
    where
        TC: TrieStorageCursor,
        HC: HashedStorageCursor<Value = U256>,
    {
        let StorageProofJob { hashed_address, mut targets, dispatched_at } = job;
        let _span = debug_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            ?hashed_address,
            targets = targets.len(),
        )
        .entered();
        let proof_start = Instant::now();

        // Without slot targets the job asks for the storage root node only.
        let proof = if targets.is_empty() {
            calculator.storage_root_node(hashed_address).map(|root_node| vec![root_node])
        } else {
            calculator.storage_proof(hashed_address, &mut targets)
        };

        trace!(
            target: "trie::proof_task",
            worker_id,
            ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            dispatch_to_completion_us = dispatched_at.elapsed().as_micros(),
            "Storage proof completed"
        );

        proof
            .map(|proof| DecodedMultiProofV2 {
                account_proofs: Vec::new(),
                storage_proofs: std::iter::once((hashed_address, proof)).collect(),
            })
            .map_err(|source| ProofJobError::Storage { hashed_address, source }.into())
    }
}

impl<Factory> ProofWorker<Factory, AccountProofJob>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    /// Opens a database transaction and serves account proof jobs until the handle is dropped.
    fn run(&self, worker_id: usize) -> ProviderResult<()> {
        let provider = self.task_ctx.factory.database_provider_ro()?;

        trace!(target: "trie::proof_task", worker_id, "Account worker started");

        #[cfg(feature = "metrics")]
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();

        // Create both account and storage calculators for V2 proofs.
        // The storage calculator is wrapped in Rc<RefCell<...>> for sharing with value encoders.
        let account_trie_cursor = provider.account_trie_cursor()?;
        let account_hashed_cursor = provider.hashed_account_cursor()?;

        let storage_trie_cursor = provider.storage_trie_cursor(B256::ZERO)?;
        let storage_hashed_cursor = provider.hashed_storage_cursor(B256::ZERO)?;

        #[cfg(feature = "metrics")]
        let account_trie_cursor = reth_trie::trie_cursor::InstrumentedTrieCursor::new(
            account_trie_cursor,
            &mut cursor_metrics_cache.account_trie_cursor,
        );
        #[cfg(feature = "metrics")]
        let account_hashed_cursor = reth_trie::hashed_cursor::InstrumentedHashedCursor::new(
            account_hashed_cursor,
            &mut cursor_metrics_cache.account_hashed_cursor,
        );
        #[cfg(feature = "metrics")]
        let storage_trie_cursor = reth_trie::trie_cursor::InstrumentedTrieCursor::new(
            storage_trie_cursor,
            &mut cursor_metrics_cache.storage_trie_cursor,
        );
        #[cfg(feature = "metrics")]
        let storage_hashed_cursor = reth_trie::hashed_cursor::InstrumentedHashedCursor::new(
            storage_hashed_cursor,
            &mut cursor_metrics_cache.storage_hashed_cursor,
        );

        let mut account_calculator = V2AccountProofCalculator::<'_, Factory::Provider>::new(
            account_trie_cursor,
            account_hashed_cursor,
        );
        let storage_calculator =
            Rc::new(RefCell::new(proof_v2::StorageProofCalculator::new_storage(
                storage_trie_cursor,
                storage_hashed_cursor,
            )));

        let mut account_proofs_processed = 0u64;
        let total_idle_time = self.process_jobs(worker_id, |job| {
            let result = Self::account_proof::<Factory::Provider>(
                &mut account_calculator,
                storage_calculator.clone(),
                job,
                worker_id,
            );
            self.send_result(worker_id, result);
            account_proofs_processed += 1;
        });

        // Drop calculators to release mutable borrows on cursor_metrics_cache.
        drop(account_calculator);
        drop(storage_calculator);

        trace!(
            target: "trie::proof_task",
            worker_id,
            account_proofs_processed,
            total_idle_time_us = total_idle_time.as_micros(),
            "Account worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.idle.record_account_worker_idle_time(total_idle_time);
            self.metrics.cursor.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Calculates the account proof for a job, calculating storage roots inline.
    fn account_proof<'a, Provider>(
        account_calculator: &mut V2AccountProofCalculator<'a, Provider>,
        storage_calculator: Rc<RefCell<V2StorageProofCalculator<'a, Provider>>>,
        job: AccountProofJob,
        worker_id: usize,
    ) -> ProofWorkerResult
    where
        Provider: TrieCursorFactory + HashedCursorFactory + 'a,
    {
        let AccountProofJob { mut targets, dispatched_at } = job;
        let _span = debug_span!(
            target: "trie::proof_task",
            "Account proof calculation",
            targets = targets.len(),
        )
        .entered();
        let proof_start = Instant::now();

        let mut value_encoder = AccountValueEncoder::new(storage_calculator);
        let proof = account_calculator.proof(&mut value_encoder, &mut targets);

        trace!(
            target: "trie::proof_task",
            worker_id,
            proof_time_us = proof_start.elapsed().as_micros(),
            dispatch_to_completion_us = dispatched_at.elapsed().as_micros(),
            "Account proof completed"
        );

        proof
            .map(|account_proofs| DecodedMultiProofV2 {
                account_proofs,
                storage_proofs: Default::default(),
            })
            .map_err(|source| ProofJobError::Account { source }.into())
    }
}

/// A queued storage proof of one account.
#[derive(Debug)]
struct StorageProofJob {
    hashed_address: B256,
    targets: Vec<ProofV2Target>,
    dispatched_at: Instant,
}

/// A queued account proof.
#[derive(Debug)]
struct AccountProofJob {
    targets: Vec<ProofV2Target>,
    dispatched_at: Instant,
}

/// Context for a provider failure that prevents a proof worker from starting.
#[derive(Debug, thiserror::Error)]
#[error("{} worker {worker_id}: {source}", kind.name())]
struct ProofWorkerError {
    kind: ProofWorkerKind,
    worker_id: usize,
    #[source]
    source: ProviderError,
}

/// A failed proof job, identified by the proof it calculated.
#[derive(Debug, thiserror::Error)]
enum ProofJobError {
    #[error("storage proof of account {hashed_address}: {source}")]
    Storage {
        hashed_address: B256,
        #[source]
        source: StateProofError,
    },
    #[error("account proof: {source}")]
    Account {
        #[source]
        source: StateProofError,
    },
}

impl From<ProofJobError> for StateRootTaskError {
    fn from(error: ProofJobError) -> Self {
        Self::Other(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainSpec;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;
    use std::sync::Arc;

    fn test_ctx<Factory>(factory: Factory) -> ProofTaskCtx<Factory> {
        ProofTaskCtx::new(factory)
    }

    /// Ensures `ProofWorkerHandle::new` spawns workers correctly.
    #[test]
    fn spawn_proof_workers_creates_handle() {
        let chain_spec = Arc::new(ChainSpec::default());
        let anchor_hash = chain_spec.genesis_hash();
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let factory = reth_storage_overlay::OverlayStateProviderFactory::new(
            provider_factory,
            reth_storage_overlay::OverlayManager::<
                reth_ethereum_primitives::EthPrimitives,
            >::default()
            .overlay_builder(anchor_hash),
        );
        let ctx = test_ctx(factory);

        let runtime = reth_tasks::Runtime::test();
        let proof_worker_handle =
            ProofWorkerHandle::new(&runtime, ctx, ProofWorkerCounts::full(&runtime));

        // Workers shut down automatically when handle is dropped
        drop(proof_worker_handle);
    }

    /// Ensures a single pool serves quarter, half, and full-capacity worker counts.
    #[test]
    fn single_pool_serves_fractional_worker_counts() {
        let provider_factory =
            create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
        let anchor_hash = reth_db_common::init::init_genesis(&provider_factory).unwrap();
        let factory = reth_storage_overlay::OverlayStateProviderFactory::new(
            provider_factory,
            reth_storage_overlay::OverlayManager::<reth_ethereum_primitives::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let ctx = test_ctx(factory);

        let runtime = reth_tasks::RuntimeBuilder::new(
            reth_tasks::RuntimeConfig::default()
                .with_rayon(reth_tasks::RayonConfig { cpu_threads: Some(2), ..Default::default() }),
        )
        .build()
        .unwrap();
        let pool_threads = runtime.proof_storage_worker_pool().num_threads();
        assert_eq!(pool_threads, 8);
        assert_eq!(runtime.proof_account_worker_pool().num_threads(), pool_threads);

        for counts in [
            ProofWorkerCounts::new(0, 0),
            ProofWorkerCounts::new(pool_threads / 4, pool_threads / 4),
            ProofWorkerCounts::new(pool_threads / 2, pool_threads / 2),
            ProofWorkerCounts::full(&runtime),
            ProofWorkerCounts::new(pool_threads + 1, pool_threads + 2),
        ] {
            let proof_worker_handle = ProofWorkerHandle::new(&runtime, ctx.clone(), counts);
            assert_eq!(
                proof_worker_handle.total_storage_workers(),
                counts.storage.max(1).min(pool_threads)
            );
            assert_eq!(
                proof_worker_handle.total_account_workers(),
                counts.account.max(1).min(pool_threads)
            );

            let address = B256::ZERO;
            let storage_targets = MultiProofTargetsV2 {
                storage_targets: std::iter::once((address, vec![ProofV2Target::new(B256::ZERO)]))
                    .collect(),
                ..Default::default()
            };
            assert_eq!(proof_worker_handle.dispatch(storage_targets).unwrap(), 1);
            let storage_proof = proof_worker_handle
                .results()
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .unwrap();
            assert!(storage_proof.account_proofs.is_empty());
            assert!(storage_proof.storage_proofs.contains_key(&address));

            let account_targets = MultiProofTargetsV2 {
                account_targets: vec![ProofV2Target::new(address)],
                ..Default::default()
            };
            assert_eq!(proof_worker_handle.dispatch(account_targets).unwrap(), 1);
            let account_proof = proof_worker_handle
                .results()
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .unwrap();
            assert!(account_proof.storage_proofs.is_empty());

            let results = proof_worker_handle.results().clone();
            drop(proof_worker_handle);
            // Every broadcast holds a result sender until all its workers have exited. An
            // initialization error is a message here, rather than a successful shutdown.
            assert!(matches!(
                results.recv_timeout(Duration::from_secs(30)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            ));
        }
    }
}
