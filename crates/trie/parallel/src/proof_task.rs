//! Parallel proof computation using worker pools with dedicated database transactions.
//!
//!
//! # Architecture
//!
//! - **Worker Pools**: Pre-spawned workers with dedicated database transactions
//!   - Storage pool: Handles storage proofs and blinded storage node requests
//!   - Account pool: Handles account multiproofs and blinded account node requests
//! - **Direct Channel Access**: [`ProofWorkerHandle`] provides type-safe queue methods with direct
//!   access to worker channels, eliminating routing overhead
//! - **Automatic Shutdown**: Workers terminate gracefully when all handles are dropped
//!
//! # Message Flow
//!
//! 1. `MultiProofTask` prepares a storage or account job and hands it to [`ProofWorkerHandle`]. The
//!    job carries a [`ProofResultContext`] so the worker knows how to send the result back.
//! 2. A worker receives the job, runs the proof, and sends a [`ProofResultMessage`] through the
//!    provided [`ProofResultSender`].
//! 3. `MultiProofTask` receives the message, uses `sequence_number` to keep proofs in order, and
//!    proceeds with its state-root logic.
//!
//! Each job gets its own direct channel so results go straight back to `MultiProofTask`. That keeps
//! ordering decisions in one place and lets workers run independently.
//!
//! ```text
//! MultiProofTask -> MultiproofManager -> ProofWorkerHandle -> Storage/Account Worker
//!        ^                                          |
//!        |                                          v
//! ProofResultMessage <-------- ProofResultSender ---
//! ```

use crate::{
    root::ParallelStateRootError,
    stats::{ParallelTrieStats, ParallelTrieTracker},
    StorageRootTargets,
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
use reth_provider::{DatabaseProviderROFactory, ProviderError, ProviderResult};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedCursorMetricsCache, InstrumentedHashedCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSets,
    proof::{ProofBlindedAccountProvider, ProofBlindedStorageProvider, StorageProof},
    proof_v2::{self, StorageProofCalculator},
    trie_cursor::{InstrumentedTrieCursor, TrieCursorFactory, TrieCursorMetricsCache},
    walker::TrieWalker,
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostState, MultiProofTargets,
    Nibbles, ProofTrieNode, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
    proof::{DecodedProofNodes, ProofRetainer},
    BranchNodeMasks, BranchNodeMasksMap,
};
use reth_trie_sparse::provider::{RevealedNode, TrieNodeProvider, TrieNodeProviderFactory};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::runtime::Handle;
use tracing::{debug, debug_span, error, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::{
    ProofTaskCursorMetrics, ProofTaskCursorMetricsCache, ProofTaskTrieMetrics,
};

type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// A handle that provides type-safe access to proof worker pools.
///
/// The handle stores direct senders to both storage and account worker pools,
/// eliminating the need for a routing thread. All handles share reference-counted
/// channels, and workers shut down gracefully when all handles are dropped.
/// Type alias for spawn function closures used in dynamic worker scaling.
type SpawnFn = Arc<dyn Fn(usize) + Send + Sync + 'static>;

/// Shared inner state for dynamic worker pool management.
struct ProofWorkerInner {
    /// Direct sender to storage worker pool
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Direct sender to account worker pool
    account_work_tx: CrossbeamSender<AccountWorkerJob>,
    /// Counter tracking available storage workers
    storage_available_workers: Arc<AtomicUsize>,
    /// Counter tracking available account workers
    account_available_workers: Arc<AtomicUsize>,
    /// Current number of storage workers (atomically updated for scaling)
    current_storage_workers: AtomicUsize,
    /// Current number of account workers (atomically updated for scaling)
    current_account_workers: AtomicUsize,
    /// Next worker ID for storage workers (monotonically increasing)
    next_storage_worker_id: AtomicUsize,
    /// Next worker ID for account workers (monotonically increasing)
    next_account_worker_id: AtomicUsize,
    /// Minimum number of storage workers
    min_storage_workers: usize,
    /// Maximum number of storage workers
    max_storage_workers: usize,
    /// Minimum number of account workers
    min_account_workers: usize,
    /// Maximum number of account workers
    max_account_workers: usize,
    /// Closure to spawn new storage workers
    storage_spawn: SpawnFn,
    /// Closure to spawn new account workers
    account_spawn: SpawnFn,
    /// Whether V2 storage proofs are enabled
    v2_proofs_enabled: bool,
    /// Workload predictor for dynamic scaling
    predictor: crate::workload_predictor::WorkloadPredictor,
}

impl std::fmt::Debug for ProofWorkerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProofWorkerInner")
            .field("current_storage_workers", &self.current_storage_workers.load(Ordering::Relaxed))
            .field("current_account_workers", &self.current_account_workers.load(Ordering::Relaxed))
            .field("min_storage_workers", &self.min_storage_workers)
            .field("max_storage_workers", &self.max_storage_workers)
            .field("min_account_workers", &self.min_account_workers)
            .field("max_account_workers", &self.max_account_workers)
            .field("v2_proofs_enabled", &self.v2_proofs_enabled)
            .finish()
    }
}

/// A handle that provides type-safe access to proof worker pools with dynamic scaling.
///
/// The handle stores direct senders to both storage and account worker pools,
/// eliminating the need for a routing thread. All handles share reference-counted
/// channels, and workers shut down gracefully when all handles are dropped.
///
/// Supports dynamic worker scaling based on workload prediction and queue pressure.
#[derive(Debug, Clone)]
pub struct ProofWorkerHandle {
    /// Shared inner state
    inner: Arc<ProofWorkerInner>,
}

/// Default multiplier for max workers relative to initial count.
const DEFAULT_MAX_WORKER_MULTIPLIER: usize = 4;

/// Number of storage slots processed per worker for scaling calculations.
const SLOTS_PER_WORKER: usize = 50;

/// Number of accounts processed per worker for scaling calculations.
const ACCOUNTS_PER_WORKER: usize = 20;

/// Maximum number of workers to spawn in a single scale-up operation.
const SCALE_UP_STEP: usize = 4;

impl ProofWorkerHandle {
    /// Spawns storage and account worker pools with dedicated database transactions.
    ///
    /// Returns a handle for submitting proof tasks to the worker pools.
    /// Workers run until the last handle is dropped.
    ///
    /// # Parameters
    /// - `executor`: Tokio runtime handle for spawning blocking tasks
    /// - `task_ctx`: Shared context with database view and prefix sets
    /// - `storage_worker_count`: Number of storage workers to spawn initially (also min)
    /// - `account_worker_count`: Number of account workers to spawn initially (also min)
    /// - `v2_proofs_enabled`: Whether to enable V2 storage proofs
    pub fn new<Factory>(
        executor: Handle,
        task_ctx: ProofTaskCtx<Factory>,
        storage_worker_count: usize,
        account_worker_count: usize,
        v2_proofs_enabled: bool,
    ) -> Self
    where
        Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();
        let (account_work_tx, account_work_rx) = unbounded::<AccountWorkerJob>();

        // Initialize availability counters at zero. Each worker will increment when it
        // successfully initializes, ensuring only healthy workers are counted.
        let storage_available_workers = Arc::new(AtomicUsize::new(0));
        let account_available_workers = Arc::new(AtomicUsize::new(0));

        let cached_storage_roots = Arc::new(DashMap::new());

        debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            ?v2_proofs_enabled,
            "Spawning proof worker pools"
        );

        // Create spawn closures that capture all necessary context for dynamic scaling
        let storage_spawn: SpawnFn = {
            let executor = executor.clone();
            let task_ctx = task_ctx.clone();
            let storage_available_workers = storage_available_workers.clone();
            let cached_storage_roots = cached_storage_roots.clone();

            Arc::new(move |worker_id: usize| {
                let span = debug_span!(target: "trie::proof_task", "storage worker", ?worker_id);
                let task_ctx_clone = task_ctx.clone();
                let work_rx_clone = storage_work_rx.clone();
                let storage_available_workers_clone = storage_available_workers.clone();
                let cached_storage_roots = cached_storage_roots.clone();

                executor.spawn_blocking(move || {
                    #[cfg(feature = "metrics")]
                    let metrics = ProofTaskTrieMetrics::default();
                    #[cfg(feature = "metrics")]
                    let cursor_metrics = ProofTaskCursorMetrics::new();

                    let _guard = span.enter();
                    let worker = StorageProofWorker::new(
                        task_ctx_clone,
                        work_rx_clone,
                        worker_id,
                        storage_available_workers_clone,
                        cached_storage_roots,
                        #[cfg(feature = "metrics")]
                        metrics,
                        #[cfg(feature = "metrics")]
                        cursor_metrics,
                    )
                    .with_v2_proofs(v2_proofs_enabled);
                    if let Err(error) = worker.run() {
                        error!(
                            target: "trie::proof_task",
                            worker_id,
                            ?error,
                            "Storage worker failed"
                        );
                    }
                });
            })
        };

        let account_spawn: SpawnFn = {
            let account_available_workers = account_available_workers.clone();
            let storage_work_tx = storage_work_tx.clone();

            Arc::new(move |worker_id: usize| {
                let span = debug_span!(target: "trie::proof_task", "account worker", ?worker_id);
                let task_ctx_clone = task_ctx.clone();
                let work_rx_clone = account_work_rx.clone();
                let storage_work_tx_clone = storage_work_tx.clone();
                let account_available_workers_clone = account_available_workers.clone();
                let cached_storage_roots = cached_storage_roots.clone();

                executor.spawn_blocking(move || {
                    #[cfg(feature = "metrics")]
                    let metrics = ProofTaskTrieMetrics::default();
                    #[cfg(feature = "metrics")]
                    let cursor_metrics = ProofTaskCursorMetrics::new();

                    let _guard = span.enter();
                    let worker = AccountProofWorker::new(
                        task_ctx_clone,
                        work_rx_clone,
                        worker_id,
                        storage_work_tx_clone,
                        account_available_workers_clone,
                        cached_storage_roots,
                        #[cfg(feature = "metrics")]
                        metrics,
                        #[cfg(feature = "metrics")]
                        cursor_metrics,
                    );
                    if let Err(error) = worker.run() {
                        error!(
                            target: "trie::proof_task",
                            worker_id,
                            ?error,
                            "Account worker failed"
                        );
                    }
                });
            })
        };

        // Spawn initial storage workers
        let parent_span =
            debug_span!(target: "trie::proof_task", "storage proof workers", ?storage_worker_count)
                .entered();
        for worker_id in 0..storage_worker_count {
            storage_spawn(worker_id);
        }
        drop(parent_span);

        // Spawn initial account workers
        let parent_span =
            debug_span!(target: "trie::proof_task", "account proof workers", ?account_worker_count)
                .entered();
        for worker_id in 0..account_worker_count {
            account_spawn(worker_id);
        }
        drop(parent_span);

        let inner = ProofWorkerInner {
            storage_work_tx,
            account_work_tx,
            storage_available_workers,
            account_available_workers,
            current_storage_workers: AtomicUsize::new(storage_worker_count),
            current_account_workers: AtomicUsize::new(account_worker_count),
            next_storage_worker_id: AtomicUsize::new(storage_worker_count),
            next_account_worker_id: AtomicUsize::new(account_worker_count),
            min_storage_workers: storage_worker_count,
            max_storage_workers: storage_worker_count * DEFAULT_MAX_WORKER_MULTIPLIER,
            min_account_workers: account_worker_count,
            max_account_workers: account_worker_count * DEFAULT_MAX_WORKER_MULTIPLIER,
            storage_spawn,
            account_spawn,
            v2_proofs_enabled,
            predictor: crate::workload_predictor::WorkloadPredictor::new(),
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns whether V2 storage proofs are enabled for this worker pool.
    pub fn v2_proofs_enabled(&self) -> bool {
        self.inner.v2_proofs_enabled
    }

    /// Returns how many storage workers are currently available/idle.
    pub fn available_storage_workers(&self) -> usize {
        self.inner.storage_available_workers.load(Ordering::Relaxed)
    }

    /// Returns how many account workers are currently available/idle.
    pub fn available_account_workers(&self) -> usize {
        self.inner.account_available_workers.load(Ordering::Relaxed)
    }

    /// Returns the number of pending storage tasks in the queue.
    pub fn pending_storage_tasks(&self) -> usize {
        self.inner.storage_work_tx.len()
    }

    /// Returns the number of pending account tasks in the queue.
    pub fn pending_account_tasks(&self) -> usize {
        self.inner.account_work_tx.len()
    }

    /// Returns the current number of storage workers in the pool.
    pub fn total_storage_workers(&self) -> usize {
        self.inner.current_storage_workers.load(Ordering::Relaxed)
    }

    /// Returns the current number of account workers in the pool.
    pub fn total_account_workers(&self) -> usize {
        self.inner.current_account_workers.load(Ordering::Relaxed)
    }

    /// Returns the number of storage workers currently processing tasks.
    ///
    /// This is calculated as total workers minus available workers.
    pub fn active_storage_workers(&self) -> usize {
        self.total_storage_workers().saturating_sub(self.available_storage_workers())
    }

    /// Returns the number of account workers currently processing tasks.
    ///
    /// This is calculated as total workers minus available workers.
    pub fn active_account_workers(&self) -> usize {
        self.total_account_workers().saturating_sub(self.available_account_workers())
    }

    /// Records block workload for prediction after block processing completes.
    pub fn record_block_workload(&self, storage_targets: usize, account_targets: usize) {
        self.inner.predictor.record(storage_targets, account_targets);
    }

    /// Returns predicted workload for the next block.
    pub fn predict_workload(&self) -> (usize, usize) {
        self.inner.predictor.predict()
    }

    /// Pre-scales workers based on predicted workload before block processing.
    pub fn prepare_for_block(&self) {
        let (predicted_storage, predicted_accounts) = self.predict_workload();

        let desired_storage = predicted_storage
            .div_ceil(SLOTS_PER_WORKER)
            .clamp(self.inner.min_storage_workers, self.inner.max_storage_workers);
        let desired_account = predicted_accounts
            .div_ceil(ACCOUNTS_PER_WORKER)
            .clamp(self.inner.min_account_workers, self.inner.max_account_workers);

        self.scale_storage_workers_to(desired_storage);
        self.scale_account_workers_to(desired_account);
    }

    /// Attempts to reserve and spawn additional storage workers up to the desired count.
    fn scale_storage_workers_to(&self, desired: usize) {
        let current = self.inner.current_storage_workers.load(Ordering::Relaxed);
        if current >= desired {
            return;
        }

        let to_spawn = self.try_reserve_workers(
            &self.inner.current_storage_workers,
            self.inner.max_storage_workers,
            (desired - current).min(SCALE_UP_STEP),
        );

        for _ in 0..to_spawn {
            let worker_id = self.inner.next_storage_worker_id.fetch_add(1, Ordering::Relaxed);
            (self.inner.storage_spawn)(worker_id);
        }

        if to_spawn > 0 {
            debug!(
                target: "trie::proof_task",
                to_spawn,
                new_total = self.inner.current_storage_workers.load(Ordering::Relaxed),
                "Scaled up storage workers"
            );
        }
    }

    /// Attempts to reserve and spawn additional account workers up to the desired count.
    fn scale_account_workers_to(&self, desired: usize) {
        let current = self.inner.current_account_workers.load(Ordering::Relaxed);
        if current >= desired {
            return;
        }

        let to_spawn = self.try_reserve_workers(
            &self.inner.current_account_workers,
            self.inner.max_account_workers,
            (desired - current).min(SCALE_UP_STEP),
        );

        for _ in 0..to_spawn {
            let worker_id = self.inner.next_account_worker_id.fetch_add(1, Ordering::Relaxed);
            (self.inner.account_spawn)(worker_id);
        }

        if to_spawn > 0 {
            debug!(
                target: "trie::proof_task",
                to_spawn,
                new_total = self.inner.current_account_workers.load(Ordering::Relaxed),
                "Scaled up account workers"
            );
        }
    }

    /// Atomically reserves workers using CAS, returns how many were reserved.
    fn try_reserve_workers(&self, current: &AtomicUsize, max: usize, n: usize) -> usize {
        loop {
            let cur = current.load(Ordering::Relaxed);
            if cur >= max {
                return 0;
            }
            let can_add = (max - cur).min(n);
            if can_add == 0 {
                return 0;
            }
            if current
                .compare_exchange(cur, cur + can_add, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return can_add;
            }
        }
    }

    /// Dispatch a storage proof computation to storage worker pool
    ///
    /// The result will be sent via the `proof_result_sender` channel.
    pub fn dispatch_storage_proof(
        &self,
        input: StorageProofInput,
        proof_result_sender: CrossbeamSender<StorageProofResultMessage>,
    ) -> Result<(), ProviderError> {
        let hashed_address = input.hashed_address();
        self.inner
            .storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("storage workers unavailable"));

                if let StorageWorkerJob::StorageProof { proof_result_sender, .. } = err.0 {
                    let _ = proof_result_sender.send(StorageProofResultMessage {
                        hashed_address,
                        result: Err(ParallelStateRootError::Provider(error.clone())),
                    });
                }

                error
            })
    }

    /// Dispatch an account multiproof computation
    ///
    /// The result will be sent via the `result_sender` channel included in the input.
    pub fn dispatch_account_multiproof(
        &self,
        input: AccountMultiproofInput,
    ) -> Result<(), ProviderError> {
        self.inner
            .account_work_tx
            .send(AccountWorkerJob::AccountMultiproof { input: Box::new(input) })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("account workers unavailable"));

                if let AccountWorkerJob::AccountMultiproof { input } = err.0 {
                    let AccountMultiproofInput {
                        proof_result_sender:
                            ProofResultContext {
                                sender: result_tx,
                                sequence_number: seq,
                                state,
                                start_time: start,
                            },
                        ..
                    } = *input;

                    let _ = result_tx.send(ProofResultMessage {
                        sequence_number: seq,
                        result: Err(ParallelStateRootError::Provider(error.clone())),
                        elapsed: start.elapsed(),
                        state,
                    });
                }

                error
            })
    }

    /// Dispatch blinded storage node request to storage worker pool
    pub(crate) fn dispatch_blinded_storage_node(
        &self,
        account: B256,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        self.inner
            .storage_work_tx
            .send(StorageWorkerJob::BlindedStorageNode { account, path, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("storage workers unavailable"))
            })?;

        Ok(rx)
    }

    /// Dispatch blinded account node request to account worker pool
    pub(crate) fn dispatch_blinded_account_node(
        &self,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        self.inner
            .account_work_tx
            .send(AccountWorkerJob::BlindedAccountNode { path, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("account workers unavailable"))
            })?;

        Ok(rx)
    }
}

/// Data used for initializing cursor factories that is shared across all proof worker instances.
#[derive(Clone, Debug)]
pub struct ProofTaskCtx<Factory> {
    /// The factory for creating state providers.
    factory: Factory,
}

impl<Factory> ProofTaskCtx<Factory> {
    /// Creates a new [`ProofTaskCtx`] with the given factory.
    pub const fn new(factory: Factory) -> Self {
        Self { factory }
    }
}

/// This contains all information shared between account proof worker instances.
#[derive(Debug)]
pub struct ProofTaskTx<Provider> {
    /// The provider that implements `TrieCursorFactory` and `HashedCursorFactory`.
    provider: Provider,

    /// Identifier for the worker within the worker pool, used only for tracing.
    id: usize,
}

impl<Provider> ProofTaskTx<Provider> {
    /// Initializes a [`ProofTaskTx`] with the given provider and ID.
    const fn new(provider: Provider, id: usize) -> Self {
        Self { provider, id }
    }
}

impl<Provider> ProofTaskTx<Provider>
where
    Provider: TrieCursorFactory + HashedCursorFactory,
{
    /// Compute storage proof.
    ///
    /// Used by storage workers in the worker pool to compute storage proofs.
    #[inline]
    fn compute_legacy_storage_proof(
        &self,
        input: StorageProofInput,
        trie_cursor_metrics: &mut TrieCursorMetricsCache,
        hashed_cursor_metrics: &mut HashedCursorMetricsCache,
    ) -> Result<StorageProofResult, ParallelStateRootError> {
        // Consume the input so we can move large collections (e.g. target slots) without cloning.
        let StorageProofInput::Legacy {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        } = input
        else {
            panic!("compute_legacy_storage_proof only accepts StorageProofInput::Legacy")
        };

        // Get or create added/removed keys context
        let multi_added_removed_keys =
            multi_added_removed_keys.unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&hashed_address);

        let span = debug_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            ?hashed_address,
            target_slots = ?target_slots.len(),
            worker_id = self.id,
        );
        let _span_guard = span.enter();

        let proof_start = Instant::now();

        // Compute raw storage multiproof
        let raw_proof_result =
            StorageProof::new_hashed(&self.provider, &self.provider, hashed_address)
                .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().copied()))
                .with_branch_node_masks(with_branch_node_masks)
                .with_added_removed_keys(added_removed_keys)
                .with_trie_cursor_metrics(trie_cursor_metrics)
                .with_hashed_cursor_metrics(hashed_cursor_metrics)
                .storage_multiproof(target_slots)
                .map_err(|e| ParallelStateRootError::Other(e.to_string()));
        trie_cursor_metrics.record_span("trie_cursor");
        hashed_cursor_metrics.record_span("hashed_cursor");

        // Decode proof into DecodedStorageMultiProof
        let decoded_result = raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    hashed_address, e
                ))
            })
        })?;

        trace!(
            target: "trie::proof_task",
            hashed_address = ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            worker_id = self.id,
            "Completed storage proof calculation"
        );

        Ok(StorageProofResult::Legacy { proof: decoded_result })
    }

    fn compute_v2_storage_proof(
        &self,
        input: StorageProofInput,
        calculator: &mut proof_v2::StorageProofCalculator<
            <Provider as TrieCursorFactory>::StorageTrieCursor<'_>,
            <Provider as HashedCursorFactory>::StorageCursor<'_>,
        >,
    ) -> Result<StorageProofResult, ParallelStateRootError> {
        let StorageProofInput::V2 { hashed_address, mut targets } = input else {
            panic!("compute_v2_storage_proof only accepts StorageProofInput::V2")
        };

        // If targets is empty it means the caller only wants the root hash. The V2 proof calculator
        // will do nothing given no targets, so instead we give it a fake target so it always
        // returns at least the root.
        if targets.is_empty() {
            targets.push(proof_v2::Target::new(B256::ZERO));
        }

        let span = debug_span!(
            target: "trie::proof_task",
            "V2 Storage proof calculation",
            ?hashed_address,
            targets = ?targets.len(),
            worker_id = self.id,
        );
        let _span_guard = span.enter();

        let proof_start = Instant::now();
        let proof = calculator.storage_proof(hashed_address, &mut targets)?;
        let root = calculator.compute_root_hash(&proof)?;

        trace!(
            target: "trie::proof_task",
            hashed_address = ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            ?root,
            worker_id = self.id,
            "Completed V2 storage proof calculation"
        );

        Ok(StorageProofResult::V2 { proof, root })
    }

    /// Process a blinded storage node request.
    ///
    /// Used by storage workers to retrieve blinded storage trie nodes for proof construction.
    fn process_blinded_storage_node(
        &self,
        account: B256,
        path: &Nibbles,
    ) -> TrieNodeProviderResult {
        let storage_node_provider =
            ProofBlindedStorageProvider::new(&self.provider, &self.provider, account);
        storage_node_provider.trie_node(path)
    }

    /// Process a blinded account node request.
    ///
    /// Used by account workers to retrieve blinded account trie nodes for proof construction.
    fn process_blinded_account_node(&self, path: &Nibbles) -> TrieNodeProviderResult {
        let account_node_provider =
            ProofBlindedAccountProvider::new(&self.provider, &self.provider);
        account_node_provider.trie_node(path)
    }
}
impl TrieNodeProviderFactory for ProofWorkerHandle {
    type AccountNodeProvider = ProofTaskTrieNodeProvider;
    type StorageNodeProvider = ProofTaskTrieNodeProvider;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { handle: self.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, handle: self.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider {
    /// Blinded account trie node provider.
    AccountNode {
        /// Handle to the proof worker pools.
        handle: ProofWorkerHandle,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Handle to the proof worker pools.
        handle: ProofWorkerHandle,
    },
}

impl TrieNodeProvider for ProofTaskTrieNodeProvider {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        match self {
            Self::AccountNode { handle } => {
                let rx = handle
                    .dispatch_blinded_account_node(*path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
            Self::StorageNode { handle, account } => {
                let rx = handle
                    .dispatch_blinded_storage_node(*account, *path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
        }
    }
}

/// Result of a multiproof calculation.
#[derive(Debug)]
pub struct ProofResult {
    /// The account multiproof
    pub proof: DecodedMultiProof,
    /// Statistics collected during proof computation
    pub stats: ParallelTrieStats,
}

/// Channel used by worker threads to deliver `ProofResultMessage` items back to
/// `MultiProofTask`.
///
/// Workers use this sender to deliver proof results directly to `MultiProofTask`.
pub type ProofResultSender = CrossbeamSender<ProofResultMessage>;

/// Message containing a completed proof result with metadata for direct delivery to
/// `MultiProofTask`.
///
/// This type enables workers to send proof results directly to the `MultiProofTask` event loop.
#[derive(Debug)]
pub struct ProofResultMessage {
    /// Sequence number for ordering proofs
    pub sequence_number: u64,
    /// The proof calculation result (either account multiproof or storage proof)
    pub result: Result<ProofResult, ParallelStateRootError>,
    /// Time taken for the entire proof calculation (from dispatch to completion)
    pub elapsed: Duration,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
}

/// Context for sending proof calculation results back to `MultiProofTask`.
///
/// This struct contains all context needed to send and track proof calculation results.
/// Workers use this to deliver completed proofs back to the main event loop.
#[derive(Debug, Clone)]
pub struct ProofResultContext {
    /// Channel sender for result delivery
    pub sender: ProofResultSender,
    /// Sequence number for proof ordering
    pub sequence_number: u64,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
    /// Calculation start time for measuring elapsed duration
    pub start_time: Instant,
}

impl ProofResultContext {
    /// Creates a new proof result context.
    pub const fn new(
        sender: ProofResultSender,
        sequence_number: u64,
        state: HashedPostState,
        start_time: Instant,
    ) -> Self {
        Self { sender, sequence_number, state, start_time }
    }
}

/// The results of a storage proof calculation.
#[derive(Debug)]
pub(crate) enum StorageProofResult {
    Legacy {
        /// The storage multiproof
        proof: DecodedStorageMultiProof,
    },
    V2 {
        /// The calculated V2 proof nodes
        proof: Vec<ProofTrieNode>,
        /// The storage root calculated by the V2 proof
        root: Option<B256>,
    },
}

impl StorageProofResult {
    /// Returns the calculated root of the trie, if one can be calculated from the proof.
    const fn root(&self) -> Option<B256> {
        match self {
            Self::Legacy { proof } => Some(proof.root),
            Self::V2 { root, .. } => *root,
        }
    }
}

impl From<StorageProofResult> for Option<DecodedStorageMultiProof> {
    /// Returns None if the V2 proof result doesn't have a calculated root hash.
    fn from(proof_result: StorageProofResult) -> Self {
        match proof_result {
            StorageProofResult::Legacy { proof } => Some(proof),
            StorageProofResult::V2 { proof, root } => root.map(|root| {
                let branch_node_masks = proof
                    .iter()
                    .filter_map(|node| node.masks.map(|masks| (node.path, masks)))
                    .collect();
                let subtree = proof.into_iter().map(|node| (node.path, node.node)).collect();
                DecodedStorageMultiProof { root, subtree, branch_node_masks }
            }),
        }
    }
}

/// Message containing a completed storage proof result with metadata.
#[derive(Debug)]
pub struct StorageProofResultMessage {
    /// The hashed address this storage proof belongs to
    pub(crate) hashed_address: B256,
    /// The storage proof calculation result
    pub(crate) result: Result<StorageProofResult, ParallelStateRootError>,
}

/// Internal message for storage workers.
#[derive(Debug)]
#[allow(dead_code)] // Shutdown variant used for future scale-down functionality
enum StorageWorkerJob {
    /// Storage proof computation request
    StorageProof {
        /// Storage proof input parameters
        input: StorageProofInput,
        /// Context for sending the proof result.
        proof_result_sender: CrossbeamSender<StorageProofResultMessage>,
    },
    /// Blinded storage node retrieval request
    BlindedStorageNode {
        /// Target account
        account: B256,
        /// Path to the storage node
        path: Nibbles,
        /// Channel to send result back to original caller
        result_sender: Sender<TrieNodeProviderResult>,
    },
    /// Graceful shutdown signal for dynamic scaling
    Shutdown,
}

/// Worker for storage trie operations.
///
/// Each worker maintains a dedicated database transaction and processes
/// storage proof requests and blinded node lookups.
struct StorageProofWorker<Factory> {
    /// Shared task context with database factory and prefix sets
    task_ctx: ProofTaskCtx<Factory>,
    /// Channel for receiving work
    work_rx: CrossbeamReceiver<StorageWorkerJob>,
    /// Unique identifier for this worker (used for tracing)
    worker_id: usize,
    /// Counter tracking worker availability
    available_workers: Arc<AtomicUsize>,
    /// Cached storage roots
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Metrics collector for this worker
    #[cfg(feature = "metrics")]
    metrics: ProofTaskTrieMetrics,
    /// Cursor metrics for this worker
    #[cfg(feature = "metrics")]
    cursor_metrics: ProofTaskCursorMetrics,
    /// Set to true if V2 proofs are enabled.
    v2_enabled: bool,
}

impl<Factory> StorageProofWorker<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    /// Creates a new storage proof worker.
    const fn new(
        task_ctx: ProofTaskCtx<Factory>,
        work_rx: CrossbeamReceiver<StorageWorkerJob>,
        worker_id: usize,
        available_workers: Arc<AtomicUsize>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
        #[cfg(feature = "metrics")] cursor_metrics: ProofTaskCursorMetrics,
    ) -> Self {
        Self {
            task_ctx,
            work_rx,
            worker_id,
            available_workers,
            cached_storage_roots,
            #[cfg(feature = "metrics")]
            metrics,
            #[cfg(feature = "metrics")]
            cursor_metrics,
            v2_enabled: false,
        }
    }

    /// Changes whether or not V2 proofs are enabled.
    const fn with_v2_proofs(mut self, v2_enabled: bool) -> Self {
        self.v2_enabled = v2_enabled;
        self
    }

    /// Runs the worker loop, processing jobs until the channel closes.
    ///
    /// # Lifecycle
    ///
    /// 1. Initializes database provider and transaction
    /// 2. Advertises availability
    /// 3. Processes jobs in a loop:
    ///    - Receives job from channel
    ///    - Marks worker as busy
    ///    - Processes the job
    ///    - Marks worker as available
    /// 4. Shuts down when channel closes
    ///
    /// # Panic Safety
    ///
    /// If this function panics, the worker thread terminates but other workers
    /// continue operating and the system degrades gracefully.
    fn run(mut self) -> ProviderResult<()> {
        // Create provider from factory
        let provider = self.task_ctx.factory.database_provider_ro()?;
        let proof_tx = ProofTaskTx::new(provider, self.worker_id);

        trace!(
            target: "trie::proof_task",
            worker_id = self.worker_id,
            "Storage worker started"
        );

        let mut storage_proofs_processed = 0u64;
        let mut storage_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();
        let mut v2_calculator = if self.v2_enabled {
            let trie_cursor = proof_tx.provider.storage_trie_cursor(B256::ZERO)?;
            let hashed_cursor = proof_tx.provider.hashed_storage_cursor(B256::ZERO)?;
            Some(proof_v2::StorageProofCalculator::new_storage(trie_cursor, hashed_cursor))
        } else {
            None
        };

        // Initially mark this worker as available.
        self.available_workers.fetch_add(1, Ordering::Relaxed);

        while let Ok(job) = self.work_rx.recv() {
            match job {
                StorageWorkerJob::Shutdown => {
                    // Worker was idle/available while waiting; decrement and exit
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    break;
                }
                StorageWorkerJob::StorageProof { input, proof_result_sender } => {
                    // Mark worker as busy.
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    self.process_storage_proof(
                        &proof_tx,
                        v2_calculator.as_mut(),
                        input,
                        proof_result_sender,
                        &mut storage_proofs_processed,
                        &mut cursor_metrics_cache,
                    );
                    // Mark worker as available again.
                    self.available_workers.fetch_add(1, Ordering::Relaxed);
                }

                StorageWorkerJob::BlindedStorageNode { account, path, result_sender } => {
                    // Mark worker as busy.
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    Self::process_blinded_node(
                        self.worker_id,
                        &proof_tx,
                        account,
                        path,
                        result_sender,
                        &mut storage_nodes_processed,
                    );
                    // Mark worker as available again.
                    self.available_workers.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        trace!(
            target: "trie::proof_task",
            worker_id = self.worker_id,
            storage_proofs_processed,
            storage_nodes_processed,
            "Storage worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.record_storage_nodes(storage_nodes_processed as usize);
            self.cursor_metrics.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Processes a storage proof request.
    fn process_storage_proof<Provider>(
        &self,
        proof_tx: &ProofTaskTx<Provider>,
        v2_calculator: Option<
            &mut StorageProofCalculator<
                <Provider as TrieCursorFactory>::StorageTrieCursor<'_>,
                <Provider as HashedCursorFactory>::StorageCursor<'_>,
            >,
        >,
        input: StorageProofInput,
        proof_result_sender: CrossbeamSender<StorageProofResultMessage>,
        storage_proofs_processed: &mut u64,
        cursor_metrics_cache: &mut ProofTaskCursorMetricsCache,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let mut trie_cursor_metrics = TrieCursorMetricsCache::default();
        let mut hashed_cursor_metrics = HashedCursorMetricsCache::default();
        let hashed_address = input.hashed_address();
        let proof_start = Instant::now();

        let result = match &input {
            StorageProofInput::Legacy { hashed_address, prefix_set, target_slots, .. } => {
                trace!(
                    target: "trie::proof_task",
                    worker_id = self.worker_id,
                    hashed_address = ?hashed_address,
                    prefix_set_len = prefix_set.len(),
                    target_slots_len = target_slots.len(),
                    "Processing storage proof"
                );

                proof_tx.compute_legacy_storage_proof(
                    input,
                    &mut trie_cursor_metrics,
                    &mut hashed_cursor_metrics,
                )
            }
            StorageProofInput::V2 { hashed_address, targets } => {
                trace!(
                    target: "trie::proof_task",
                    worker_id = self.worker_id,
                    hashed_address = ?hashed_address,
                    targets_len = targets.len(),
                    "Processing V2 storage proof"
                );
                proof_tx
                    .compute_v2_storage_proof(input, v2_calculator.expect("v2 calculator provided"))
            }
        };

        let proof_elapsed = proof_start.elapsed();
        *storage_proofs_processed += 1;

        let root = result.as_ref().ok().and_then(|result| result.root());

        if proof_result_sender.send(StorageProofResultMessage { hashed_address, result }).is_err() {
            trace!(
                target: "trie::proof_task",
                worker_id = self.worker_id,
                hashed_address = ?hashed_address,
                storage_proofs_processed,
                "Proof result receiver dropped, discarding result"
            );
        }

        if let Some(root) = root {
            self.cached_storage_roots.insert(hashed_address, root);
        }

        trace!(
            target: "trie::proof_task",
            worker_id = self.worker_id,
            hashed_address = ?hashed_address,
            proof_time_us = proof_elapsed.as_micros(),
            total_processed = storage_proofs_processed,
            trie_cursor_duration_us = trie_cursor_metrics.total_duration.as_micros(),
            hashed_cursor_duration_us = hashed_cursor_metrics.total_duration.as_micros(),
            ?trie_cursor_metrics,
            ?hashed_cursor_metrics,
            ?root,
            "Storage proof completed"
        );

        #[cfg(feature = "metrics")]
        {
            // Accumulate per-proof metrics into the worker's cache
            let per_proof_cache = ProofTaskCursorMetricsCache {
                account_trie_cursor: TrieCursorMetricsCache::default(),
                account_hashed_cursor: HashedCursorMetricsCache::default(),
                storage_trie_cursor: trie_cursor_metrics,
                storage_hashed_cursor: hashed_cursor_metrics,
            };
            cursor_metrics_cache.extend(&per_proof_cache);
        }
    }

    /// Processes a blinded storage node lookup request.
    fn process_blinded_node<Provider>(
        worker_id: usize,
        proof_tx: &ProofTaskTx<Provider>,
        account: B256,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
        storage_nodes_processed: &mut u64,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        trace!(
            target: "trie::proof_task",
            worker_id,
            ?account,
            ?path,
            "Processing blinded storage node"
        );

        let start = Instant::now();
        let result = proof_tx.process_blinded_storage_node(account, &path);
        let elapsed = start.elapsed();

        *storage_nodes_processed += 1;

        if result_sender.send(result).is_err() {
            trace!(
                target: "trie::proof_task",
                worker_id,
                ?account,
                ?path,
                storage_nodes_processed,
                "Blinded storage node receiver dropped, discarding result"
            );
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            ?account,
            ?path,
            elapsed_us = elapsed.as_micros(),
            total_processed = storage_nodes_processed,
            "Blinded storage node completed"
        );
    }
}

/// Worker for account trie operations.
///
/// Each worker maintains a dedicated database transaction and processes
/// account multiproof requests and blinded node lookups.
struct AccountProofWorker<Factory> {
    /// Shared task context with database factory and prefix sets
    task_ctx: ProofTaskCtx<Factory>,
    /// Channel for receiving work
    work_rx: CrossbeamReceiver<AccountWorkerJob>,
    /// Unique identifier for this worker (used for tracing)
    worker_id: usize,
    /// Channel for dispatching storage proof work
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Counter tracking worker availability
    available_workers: Arc<AtomicUsize>,
    /// Cached storage roots
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Metrics collector for this worker
    #[cfg(feature = "metrics")]
    metrics: ProofTaskTrieMetrics,
    /// Cursor metrics for this worker
    #[cfg(feature = "metrics")]
    cursor_metrics: ProofTaskCursorMetrics,
}

impl<Factory> AccountProofWorker<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    /// Creates a new account proof worker.
    #[allow(clippy::too_many_arguments)]
    const fn new(
        task_ctx: ProofTaskCtx<Factory>,
        work_rx: CrossbeamReceiver<AccountWorkerJob>,
        worker_id: usize,
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        available_workers: Arc<AtomicUsize>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
        #[cfg(feature = "metrics")] cursor_metrics: ProofTaskCursorMetrics,
    ) -> Self {
        Self {
            task_ctx,
            work_rx,
            worker_id,
            storage_work_tx,
            available_workers,
            cached_storage_roots,
            #[cfg(feature = "metrics")]
            metrics,
            #[cfg(feature = "metrics")]
            cursor_metrics,
        }
    }

    /// Runs the worker loop, processing jobs until the channel closes.
    ///
    /// # Lifecycle
    ///
    /// 1. Initializes database provider and transaction
    /// 2. Advertises availability
    /// 3. Processes jobs in a loop:
    ///    - Receives job from channel
    ///    - Marks worker as busy
    ///    - Processes the job
    ///    - Marks worker as available
    /// 4. Shuts down when channel closes
    ///
    /// # Panic Safety
    ///
    /// If this function panics, the worker thread terminates but other workers
    /// continue operating and the system degrades gracefully.
    fn run(mut self) -> ProviderResult<()> {
        // Create provider from factory
        let provider = self.task_ctx.factory.database_provider_ro()?;
        let proof_tx = ProofTaskTx::new(provider, self.worker_id);

        trace!(
            target: "trie::proof_task",
            worker_id=self.worker_id,
            "Account worker started"
        );

        let mut account_proofs_processed = 0u64;
        let mut account_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();

        // Count this worker as available only after successful initialization.
        self.available_workers.fetch_add(1, Ordering::Relaxed);

        while let Ok(job) = self.work_rx.recv() {
            match job {
                AccountWorkerJob::Shutdown => {
                    // Worker was idle/available while waiting; decrement and exit
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    break;
                }
                AccountWorkerJob::AccountMultiproof { input } => {
                    // Mark worker as busy.
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    self.process_account_multiproof(
                        &proof_tx,
                        *input,
                        &mut account_proofs_processed,
                        &mut cursor_metrics_cache,
                    );
                    // Mark worker as available again.
                    self.available_workers.fetch_add(1, Ordering::Relaxed);
                }

                AccountWorkerJob::BlindedAccountNode { path, result_sender } => {
                    // Mark worker as busy.
                    self.available_workers.fetch_sub(1, Ordering::Relaxed);
                    Self::process_blinded_node(
                        self.worker_id,
                        &proof_tx,
                        path,
                        result_sender,
                        &mut account_nodes_processed,
                    );
                    // Mark worker as available again.
                    self.available_workers.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        trace!(
            target: "trie::proof_task",
            worker_id=self.worker_id,
            account_proofs_processed,
            account_nodes_processed,
            "Account worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.record_account_nodes(account_nodes_processed as usize);
            self.cursor_metrics.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Processes an account multiproof request.
    fn process_account_multiproof<Provider>(
        &self,
        proof_tx: &ProofTaskTx<Provider>,
        input: AccountMultiproofInput,
        account_proofs_processed: &mut u64,
        cursor_metrics_cache: &mut ProofTaskCursorMetricsCache,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let AccountMultiproofInput {
            targets,
            mut prefix_sets,
            collect_branch_node_masks,
            multi_added_removed_keys,
            proof_result_sender:
                ProofResultContext { sender: result_tx, sequence_number: seq, state, start_time: start },
            v2_proofs_enabled,
        } = input;

        let span = debug_span!(
            target: "trie::proof_task",
            "Account multiproof calculation",
            targets = targets.len(),
            worker_id=self.worker_id,
        );
        let _span_guard = span.enter();

        trace!(
            target: "trie::proof_task",
            "Processing account multiproof"
        );

        let proof_start = Instant::now();

        let mut tracker = ParallelTrieTracker::default();

        let mut storage_prefix_sets = std::mem::take(&mut prefix_sets.storage_prefix_sets);

        let storage_root_targets_len =
            StorageRootTargets::count(&prefix_sets.account_prefix_set, &storage_prefix_sets);

        tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

        let storage_proof_receivers = match dispatch_storage_proofs(
            &self.storage_work_tx,
            &targets,
            &mut storage_prefix_sets,
            collect_branch_node_masks,
            multi_added_removed_keys.as_ref(),
            v2_proofs_enabled,
        ) {
            Ok(receivers) => receivers,
            Err(error) => {
                // Send error through result channel
                error!(target: "trie::proof_task", "Failed to dispatch storage proofs: {error}");
                let _ = result_tx.send(ProofResultMessage {
                    sequence_number: seq,
                    result: Err(error),
                    elapsed: start.elapsed(),
                    state,
                });
                return;
            }
        };

        // Use the missed leaves cache passed from the multiproof manager
        let account_prefix_set = std::mem::take(&mut prefix_sets.account_prefix_set);

        let ctx = AccountMultiproofParams {
            targets: &targets,
            prefix_set: account_prefix_set,
            collect_branch_node_masks,
            multi_added_removed_keys: multi_added_removed_keys.as_ref(),
            storage_proof_receivers,
            cached_storage_roots: &self.cached_storage_roots,
        };

        let result =
            build_account_multiproof_with_storage_roots(&proof_tx.provider, ctx, &mut tracker);

        let now = Instant::now();
        let proof_elapsed = now.duration_since(proof_start);
        let total_elapsed = now.duration_since(start);
        let proof_cursor_metrics = tracker.cursor_metrics;
        proof_cursor_metrics.record_spans();

        let stats = tracker.finish();
        let result = result.map(|proof| ProofResult { proof, stats });
        *account_proofs_processed += 1;

        // Send result to MultiProofTask
        if result_tx
            .send(ProofResultMessage {
                sequence_number: seq,
                result,
                elapsed: total_elapsed,
                state,
            })
            .is_err()
        {
            trace!(
                target: "trie::proof_task",
                worker_id=self.worker_id,
                account_proofs_processed,
                "Account multiproof receiver dropped, discarding result"
            );
        }

        trace!(
            target: "trie::proof_task",
            proof_time_us = proof_elapsed.as_micros(),
            total_elapsed_us = total_elapsed.as_micros(),
            total_processed = account_proofs_processed,
            account_trie_cursor_duration_us = proof_cursor_metrics.account_trie_cursor.total_duration.as_micros(),
            account_hashed_cursor_duration_us = proof_cursor_metrics.account_hashed_cursor.total_duration.as_micros(),
            storage_trie_cursor_duration_us = proof_cursor_metrics.storage_trie_cursor.total_duration.as_micros(),
            storage_hashed_cursor_duration_us = proof_cursor_metrics.storage_hashed_cursor.total_duration.as_micros(),
            account_trie_cursor_metrics = ?proof_cursor_metrics.account_trie_cursor,
            account_hashed_cursor_metrics = ?proof_cursor_metrics.account_hashed_cursor,
            storage_trie_cursor_metrics = ?proof_cursor_metrics.storage_trie_cursor,
            storage_hashed_cursor_metrics = ?proof_cursor_metrics.storage_hashed_cursor,
            "Account multiproof completed"
        );

        #[cfg(feature = "metrics")]
        // Accumulate per-proof metrics into the worker's cache
        cursor_metrics_cache.extend(&proof_cursor_metrics);
    }

    /// Processes a blinded account node lookup request.
    fn process_blinded_node<Provider>(
        worker_id: usize,
        proof_tx: &ProofTaskTx<Provider>,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
        account_nodes_processed: &mut u64,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let span = debug_span!(
            target: "trie::proof_task",
            "Blinded account node calculation",
            ?path,
            worker_id,
        );
        let _span_guard = span.enter();

        trace!(
            target: "trie::proof_task",
            "Processing blinded account node"
        );

        let start = Instant::now();
        let result = proof_tx.process_blinded_account_node(&path);
        let elapsed = start.elapsed();

        *account_nodes_processed += 1;

        if result_sender.send(result).is_err() {
            trace!(
                target: "trie::proof_task",
                worker_id,
                ?path,
                account_nodes_processed,
                "Blinded account node receiver dropped, discarding result"
            );
        }

        trace!(
            target: "trie::proof_task",
            node_time_us = elapsed.as_micros(),
            total_processed = account_nodes_processed,
            "Blinded account node completed"
        );
    }
}

/// Builds an account multiproof by consuming storage proof receivers lazily during trie walk.
///
/// This is a helper function used by account workers to build the account subtree proof
/// while storage proofs are still being computed. Receivers are consumed only when needed,
/// enabling interleaved parallelism between account trie traversal and storage proof computation.
///
/// Returns a `DecodedMultiProof` containing the account subtree and storage proofs.
fn build_account_multiproof_with_storage_roots<P>(
    provider: &P,
    ctx: AccountMultiproofParams<'_>,
    tracker: &mut ParallelTrieTracker,
) -> Result<DecodedMultiProof, ParallelStateRootError>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    let accounts_added_removed_keys =
        ctx.multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

    // Create local metrics caches for account cursors. We can't directly use the metrics caches in
    // the tracker due to the call to `inc_missed_leaves` which occurs on it.
    let mut account_trie_cursor_metrics = TrieCursorMetricsCache::default();
    let mut account_hashed_cursor_metrics = HashedCursorMetricsCache::default();

    // Wrap account trie cursor with instrumented cursor
    let account_trie_cursor = provider.account_trie_cursor().map_err(ProviderError::Database)?;
    let account_trie_cursor =
        InstrumentedTrieCursor::new(account_trie_cursor, &mut account_trie_cursor_metrics);

    // Create the walker.
    let walker = TrieWalker::<_>::state_trie(account_trie_cursor, ctx.prefix_set)
        .with_added_removed_keys(accounts_added_removed_keys)
        .with_deletions_retained(true);

    // Create a hash builder to rebuild the root node since it is not available in the database.
    let retainer = ctx
        .targets
        .keys()
        .map(Nibbles::unpack)
        .collect::<ProofRetainer>()
        .with_added_removed_keys(accounts_added_removed_keys);
    let mut hash_builder = HashBuilder::default()
        .with_proof_retainer(retainer)
        .with_updates(ctx.collect_branch_node_masks);

    // Initialize storage multiproofs map with pre-allocated capacity.
    // Proofs will be inserted as they're consumed from receivers during trie walk.
    let mut collected_decoded_storages: B256Map<DecodedStorageMultiProof> =
        B256Map::with_capacity_and_hasher(ctx.targets.len(), Default::default());
    let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);

    // Wrap account hashed cursor with instrumented cursor
    let account_hashed_cursor =
        provider.hashed_account_cursor().map_err(ProviderError::Database)?;
    let account_hashed_cursor =
        InstrumentedHashedCursor::new(account_hashed_cursor, &mut account_hashed_cursor_metrics);

    let mut account_node_iter = TrieNodeIter::state_trie(walker, account_hashed_cursor);

    let mut storage_proof_receivers = ctx.storage_proof_receivers;

    while let Some(account_node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
        match account_node {
            TrieElement::Branch(node) => {
                hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
            }
            TrieElement::Leaf(hashed_address, account) => {
                let root = match storage_proof_receivers.remove(&hashed_address) {
                    Some(receiver) => {
                        let _guard = debug_span!(
                            target: "trie::proof_task",
                            "Waiting for storage proof",
                            ?hashed_address,
                        );
                        // Block on this specific storage proof receiver - enables interleaved
                        // parallelism
                        let proof_msg = receiver.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(
                                reth_execution_errors::StorageRootError::Database(
                                    DatabaseError::Other(format!(
                                        "Storage proof channel closed for {hashed_address}"
                                    )),
                                ),
                            )
                        })?;

                        drop(_guard);

                        // Extract storage proof from the result
                        debug_assert_eq!(
                            proof_msg.hashed_address, hashed_address,
                            "storage worker must return same address"
                        );
                        let proof_result = proof_msg.result?;
                        let Some(root) = proof_result.root() else {
                            trace!(
                                target: "trie::proof_task",
                                ?proof_result,
                                "Received proof_result without root",
                            );
                            panic!("Partial proofs are not yet supported");
                        };
                        let proof = Into::<Option<DecodedStorageMultiProof>>::into(proof_result)
                            .expect("Partial proofs are not yet supported (into)");
                        collected_decoded_storages.insert(hashed_address, proof);
                        root
                    }
                    // Since we do not store all intermediate nodes in the database, there might
                    // be a possibility of re-adding a non-modified leaf to the hash builder.
                    None => {
                        tracker.inc_missed_leaves();

                        match ctx.cached_storage_roots.entry(hashed_address) {
                            dashmap::Entry::Occupied(occ) => *occ.get(),
                            dashmap::Entry::Vacant(vac) => {
                                let root =
                                    StorageProof::new_hashed(provider, provider, hashed_address)
                                        .with_prefix_set_mut(Default::default())
                                        .with_trie_cursor_metrics(
                                            &mut tracker.cursor_metrics.storage_trie_cursor,
                                        )
                                        .with_hashed_cursor_metrics(
                                            &mut tracker.cursor_metrics.storage_hashed_cursor,
                                        )
                                        .storage_multiproof(
                                            ctx.targets
                                                .get(&hashed_address)
                                                .cloned()
                                                .unwrap_or_default(),
                                        )
                                        .map_err(|e| {
                                            ParallelStateRootError::StorageRoot(
                                                reth_execution_errors::StorageRootError::Database(
                                                    DatabaseError::Other(e.to_string()),
                                                ),
                                            )
                                        })?
                                        .root;

                                vac.insert(root);
                                root
                            }
                        }
                    }
                };

                // Encode account
                account_rlp.clear();
                let account = account.into_trie_account(root);
                account.encode(&mut account_rlp as &mut dyn BufMut);

                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
            }
        }
    }

    let _ = hash_builder.root();

    let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
    let decoded_account_subtree = DecodedProofNodes::try_from(account_subtree_raw_nodes)?;

    let branch_node_masks = if ctx.collect_branch_node_masks {
        let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
        updated_branch_nodes
            .into_iter()
            .map(|(path, node)| {
                (path, BranchNodeMasks { hash_mask: node.hash_mask, tree_mask: node.tree_mask })
            })
            .collect()
    } else {
        BranchNodeMasksMap::default()
    };

    // Extend tracker with accumulated metrics from account cursors
    tracker.cursor_metrics.account_trie_cursor.extend(&account_trie_cursor_metrics);
    tracker.cursor_metrics.account_hashed_cursor.extend(&account_hashed_cursor_metrics);

    // Consume remaining storage proof receivers for accounts not encountered during trie walk.
    // Done last to allow storage workers more time to complete while we finalized the account trie.
    for (hashed_address, receiver) in storage_proof_receivers {
        if let Ok(proof_msg) = receiver.recv() {
            let proof_result = proof_msg.result?;
            let proof = Into::<Option<DecodedStorageMultiProof>>::into(proof_result)
                .expect("Partial proofs are not yet supported");
            collected_decoded_storages.insert(hashed_address, proof);
        }
    }

    Ok(DecodedMultiProof {
        account_subtree: decoded_account_subtree,
        branch_node_masks,
        storages: collected_decoded_storages,
    })
}
/// Queues storage proofs for all accounts in the targets and returns receivers.
///
/// This function queues all storage proof tasks to the worker pool but returns immediately
/// with receivers, allowing the account trie walk to proceed in parallel with storage proof
/// computation. This enables interleaved parallelism for better performance.
///
/// Propagates errors up if queuing fails. Receivers must be consumed by the caller.
fn dispatch_storage_proofs(
    storage_work_tx: &CrossbeamSender<StorageWorkerJob>,
    targets: &MultiProofTargets,
    storage_prefix_sets: &mut B256Map<PrefixSet>,
    with_branch_node_masks: bool,
    multi_added_removed_keys: Option<&Arc<MultiAddedRemovedKeys>>,
    use_v2_proofs: bool,
) -> Result<B256Map<CrossbeamReceiver<StorageProofResultMessage>>, ParallelStateRootError> {
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());

    // Dispatch all storage proofs to worker pool
    for (hashed_address, target_slots) in targets.iter() {
        // Create channel for receiving ProofResultMessage
        let (result_tx, result_rx) = crossbeam_channel::unbounded();

        // Create computation input based on V2 flag
        let input = if use_v2_proofs {
            // Convert target slots to V2 targets
            let v2_targets = target_slots.iter().copied().map(Into::into).collect();
            StorageProofInput::new(*hashed_address, v2_targets)
        } else {
            let prefix_set = storage_prefix_sets.remove(hashed_address).unwrap_or_default();
            StorageProofInput::legacy(
                *hashed_address,
                prefix_set,
                target_slots.clone(),
                with_branch_node_masks,
                multi_added_removed_keys.cloned(),
            )
        };

        // Always dispatch a storage proof so we obtain the storage root even when no slots are
        // requested.
        storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender: result_tx })
            .map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "Failed to queue storage proof for {}: storage worker pool unavailable",
                    hashed_address
                ))
            })?;

        storage_proof_receivers.insert(*hashed_address, result_rx);
    }

    Ok(storage_proof_receivers)
}
/// Input parameters for storage proof computation.
#[derive(Debug)]
pub enum StorageProofInput {
    /// Legacy storage proof variant
    Legacy {
        /// The hashed address for which the proof is calculated.
        hashed_address: B256,
        /// The prefix set for the proof calculation.
        prefix_set: PrefixSet,
        /// The target slots for the proof calculation.
        target_slots: B256Set,
        /// Whether or not to collect branch node masks
        with_branch_node_masks: bool,
        /// Provided by the user to give the necessary context to retain extra proofs.
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    },
    /// V2 storage proof variant
    V2 {
        /// The hashed address for which the proof is calculated.
        hashed_address: B256,
        /// The set of proof targets
        targets: Vec<proof_v2::Target>,
    },
}

impl StorageProofInput {
    /// Creates a legacy [`StorageProofInput`] with the given hashed address, prefix set, and target
    /// slots.
    pub const fn legacy(
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: B256Set,
        with_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    ) -> Self {
        Self::Legacy {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        }
    }

    /// Creates a new [`StorageProofInput`] with the given hashed address  and target slots.
    pub const fn new(hashed_address: B256, targets: Vec<proof_v2::Target>) -> Self {
        Self::V2 { hashed_address, targets }
    }

    /// Returns the targeted hashed address.
    pub const fn hashed_address(&self) -> B256 {
        match self {
            Self::Legacy { hashed_address, .. } | Self::V2 { hashed_address, .. } => {
                *hashed_address
            }
        }
    }
}

/// Input parameters for account multiproof computation.
#[derive(Debug, Clone)]
pub struct AccountMultiproofInput {
    /// The targets for which to compute the multiproof.
    pub targets: MultiProofTargets,
    /// The prefix sets for the proof calculation.
    pub prefix_sets: TriePrefixSets,
    /// Whether or not to collect branch node masks.
    pub collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Context for sending the proof result.
    pub proof_result_sender: ProofResultContext,
    /// Whether to use V2 storage proofs.
    pub v2_proofs_enabled: bool,
}

/// Parameters for building an account multiproof with pre-computed storage roots.
struct AccountMultiproofParams<'a> {
    /// The targets for which to compute the multiproof.
    targets: &'a MultiProofTargets,
    /// The prefix set for the account trie walk.
    prefix_set: PrefixSet,
    /// Whether or not to collect branch node masks.
    collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<&'a Arc<MultiAddedRemovedKeys>>,
    /// Receivers for storage proofs being computed in parallel.
    storage_proof_receivers: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    /// Cached storage roots. This will be used to read storage roots for missed leaves, as well as
    /// to write calculated storage roots.
    cached_storage_roots: &'a DashMap<B256, B256>,
}

/// Internal message for account workers.
#[derive(Debug)]
#[allow(dead_code)] // Shutdown variant used for future scale-down functionality
enum AccountWorkerJob {
    /// Account multiproof computation request
    AccountMultiproof {
        /// Account multiproof input parameters
        input: Box<AccountMultiproofInput>,
    },
    /// Blinded account node retrieval request
    BlindedAccountNode {
        /// Path to the account node
        path: Nibbles,
        /// Channel to send result back to original caller
        result_sender: Sender<TrieNodeProviderResult>,
    },
    /// Graceful shutdown signal for dynamic scaling
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::test_utils::create_test_provider_factory;
    use tokio::{runtime::Builder, task};

    fn test_ctx<Factory>(factory: Factory) -> ProofTaskCtx<Factory> {
        ProofTaskCtx::new(factory)
    }

    /// Ensures `ProofWorkerHandle::new` spawns workers correctly.
    #[test]
    fn spawn_proof_workers_creates_handle() {
        let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = tokio::runtime::Handle::current();
            let provider_factory = create_test_provider_factory();
            let changeset_cache = reth_trie_db::ChangesetCache::new();
            let factory = reth_provider::providers::OverlayStateProviderFactory::new(
                provider_factory,
                changeset_cache,
            );
            let ctx = test_ctx(factory);

            let proof_handle = ProofWorkerHandle::new(handle.clone(), ctx, 5, 3, false);

            // Verify handle can be cloned
            let _cloned_handle = proof_handle.clone();

            // Workers shut down automatically when handle is dropped
            drop(proof_handle);
            task::yield_now().await;
        });
    }
}
