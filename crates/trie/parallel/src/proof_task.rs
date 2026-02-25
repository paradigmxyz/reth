//! Parallel proof computation using worker pools with dedicated database transactions.
//!
//!
//! # Architecture
//!
//! - **Worker Pools**: Pre-spawned workers with dedicated database transactions
//!   - Storage pool: Handles storage proofs and blinded storage node requests
//!   - Account pool: Handles account multiproofs and blinded account node requests
//! - **Direct Channel Access**: `ProofWorkerHandle` provides type-safe queue methods with direct
//!   access to worker channels, eliminating routing overhead
//! - **Automatic Shutdown**: Workers terminate gracefully when all handles are dropped
//!
//! # Message Flow
//!
//! 1. The multiproof task prepares a storage or account job and hands it to `ProofWorkerHandle`.
//!    The job carries a `ProofResultContext` so the worker knows how to send the result back.
//! 2. A worker receives the job, runs the proof, and sends a `ProofResultMessage` through the
//!    provided `ProofResultSender`.
//! 3. The multiproof task receives the message and proceeds with its state-root logic.
//!
//! Each job gets its own direct channel so results go straight back to the multiproof task. That
//! keeps ordering decisions in one place and lets workers run independently.
//!
//! ```text
//! SparseTrieCacheTask -> ProofWorkerHandle -> Storage/Account Worker
//!        ^                       |
//!        |                       v
//! ProofResultMessage <-- ProofResultSender
//! ```

use crate::{
    root::ParallelStateRootError,
    targets_v2::MultiProofTargetsV2,
    value_encoder::{AsyncAccountValueEncoder, ValueEncoderStats},
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind, StateProofError};
use reth_primitives_traits::{dashmap::DashMap, FastInstant as Instant};
use reth_provider::{DatabaseProviderROFactory, ProviderError, ProviderResult};
use reth_storage_errors::db::DatabaseError;
use reth_tasks::Runtime;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    proof::{ProofBlindedAccountProvider, ProofBlindedStorageProvider},
    proof_v2,
    trie_cursor::TrieCursorFactory,
    DecodedMultiProofV2, HashedPostState, Nibbles, ProofTrieNodeV2,
};
use reth_trie_sparse::provider::{RevealedNode, TrieNodeProvider, TrieNodeProviderFactory};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, debug_span, error, instrument, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::{
    ProofTaskCursorMetrics, ProofTaskCursorMetricsCache, ProofTaskTrieMetrics,
};

type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Type alias for the V2 account proof calculator.
type V2AccountProofCalculator<'a, Provider> = proof_v2::ProofCalculator<
    <Provider as TrieCursorFactory>::AccountTrieCursor<'a>,
    <Provider as HashedCursorFactory>::AccountCursor<'a>,
    AsyncAccountValueEncoder<
        <Provider as TrieCursorFactory>::StorageTrieCursor<'a>,
        <Provider as HashedCursorFactory>::StorageCursor<'a>,
    >,
>;

/// Type alias for the V2 storage proof calculator.
type V2StorageProofCalculator<'a, Provider> = proof_v2::StorageProofCalculator<
    <Provider as TrieCursorFactory>::StorageTrieCursor<'a>,
    <Provider as HashedCursorFactory>::StorageCursor<'a>,
>;

/// A handle that provides type-safe access to proof worker pools.
///
/// The handle stores direct senders to both storage and account worker pools,
/// eliminating the need for a routing thread. All handles share reference-counted
/// channels, and workers shut down gracefully when all handles are dropped.
#[derive(Debug, Clone)]
pub struct ProofWorkerHandle {
    /// Direct sender to storage worker pool
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Direct sender to account worker pool
    account_work_tx: CrossbeamSender<AccountWorkerJob>,
    /// Counter tracking available storage workers. Workers decrement when starting work,
    /// increment when finishing. Used to determine whether to chunk multiproofs.
    storage_available_workers: Arc<AtomicUsize>,
    /// Counter tracking available account workers. Workers decrement when starting work,
    /// increment when finishing. Used to determine whether to chunk multiproofs.
    account_available_workers: Arc<AtomicUsize>,
    /// Total number of storage workers spawned
    storage_worker_count: usize,
    /// Total number of account workers spawned
    account_worker_count: usize,
}

impl ProofWorkerHandle {
    /// Spawns storage and account worker pools with dedicated database transactions.
    ///
    /// Returns a handle for submitting proof tasks to the worker pools.
    /// Workers run until the last handle is dropped.
    ///
    /// # Parameters
    /// - `runtime`: The centralized runtime used to spawn blocking worker tasks
    /// - `task_ctx`: Shared context with database view and prefix sets
    /// - `halve_workers`: Whether to halve the worker pool size (for small blocks)
    #[instrument(
        name = "ProofWorkerHandle::new",
        level = "debug",
        target = "trie::proof_task",
        skip_all
    )]
    pub fn new<Factory>(
        runtime: &Runtime,
        task_ctx: ProofTaskCtx<Factory>,
        halve_workers: bool,
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

        let storage_available_workers = Arc::<AtomicUsize>::default();
        let account_available_workers = Arc::<AtomicUsize>::default();

        let cached_storage_roots = Arc::<DashMap<_, _>>::default();

        let divisor = if halve_workers { 2 } else { 1 };
        let storage_worker_count =
            runtime.proof_storage_worker_pool().current_num_threads() / divisor;
        let account_worker_count =
            runtime.proof_account_worker_pool().current_num_threads() / divisor;

        debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            halve_workers,
            "Spawning proof worker pools"
        );

        // broadcast blocks until all workers exit (channel close), so run on
        // tokio's blocking pool.
        let storage_rt = runtime.clone();
        let storage_task_ctx = task_ctx.clone();
        let storage_avail = storage_available_workers.clone();
        let storage_roots = cached_storage_roots.clone();
        let storage_parent_span = tracing::Span::current();
        runtime.spawn_blocking(move || {
            let worker_id = AtomicUsize::new(0);
            storage_rt.proof_storage_worker_pool().broadcast(storage_worker_count, |_| {
                let worker_id = worker_id.fetch_add(1, Ordering::Relaxed);
                let span = debug_span!(target: "trie::proof_task", parent: storage_parent_span.clone(), "storage_worker", ?worker_id);
                let _guard = span.enter();

                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();
                #[cfg(feature = "metrics")]
                let cursor_metrics = ProofTaskCursorMetrics::new();

                let worker = StorageProofWorker::new(
                    storage_task_ctx.clone(),
                    storage_work_rx.clone(),
                    worker_id,
                    storage_avail.clone(),
                    storage_roots.clone(),
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
                        "Storage worker failed"
                    );
                }
            });
        });

        let account_rt = runtime.clone();
        let account_tx = storage_work_tx.clone();
        let account_avail = account_available_workers.clone();
        let account_parent_span = tracing::Span::current();
        runtime.spawn_blocking(move || {
            let worker_id = AtomicUsize::new(0);
            account_rt.proof_account_worker_pool().broadcast(account_worker_count, |_| {
                let worker_id = worker_id.fetch_add(1, Ordering::Relaxed);
                let span = debug_span!(target: "trie::proof_task", parent: account_parent_span.clone(), "account_worker", ?worker_id);
                let _guard = span.enter();

                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();
                #[cfg(feature = "metrics")]
                let cursor_metrics = ProofTaskCursorMetrics::new();

                let worker = AccountProofWorker::new(
                    task_ctx.clone(),
                    account_work_rx.clone(),
                    worker_id,
                    account_tx.clone(),
                    account_avail.clone(),
                    cached_storage_roots.clone(),
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
        });

        Self {
            storage_work_tx,
            account_work_tx,
            storage_available_workers,
            account_available_workers,
            storage_worker_count,
            account_worker_count,
        }
    }

    /// Returns how many storage workers are currently available/idle.
    pub fn available_storage_workers(&self) -> usize {
        self.storage_available_workers.load(Ordering::Relaxed)
    }

    /// Returns how many account workers are currently available/idle.
    pub fn available_account_workers(&self) -> usize {
        self.account_available_workers.load(Ordering::Relaxed)
    }

    /// Returns the number of pending storage tasks in the queue.
    pub fn pending_storage_tasks(&self) -> usize {
        self.storage_work_tx.len()
    }

    /// Returns the number of pending account tasks in the queue.
    pub fn pending_account_tasks(&self) -> usize {
        self.account_work_tx.len()
    }

    /// Returns the total number of storage workers in the pool.
    pub const fn total_storage_workers(&self) -> usize {
        self.storage_worker_count
    }

    /// Returns the total number of account workers in the pool.
    pub const fn total_account_workers(&self) -> usize {
        self.account_worker_count
    }

    /// Returns the number of storage workers currently processing tasks.
    ///
    /// This is calculated as total workers minus available workers.
    pub fn active_storage_workers(&self) -> usize {
        self.storage_worker_count.saturating_sub(self.available_storage_workers())
    }

    /// Returns the number of account workers currently processing tasks.
    ///
    /// This is calculated as total workers minus available workers.
    pub fn active_account_workers(&self) -> usize {
        self.account_worker_count.saturating_sub(self.available_account_workers())
    }

    /// Dispatch a storage proof computation to storage worker pool
    ///
    /// The result will be sent via the `proof_result_sender` channel.
    pub fn dispatch_storage_proof(
        &self,
        input: StorageProofInput,
        proof_result_sender: CrossbeamSender<StorageProofResultMessage>,
    ) -> Result<(), ProviderError> {
        let hashed_address = input.hashed_address;
        self.storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender })
            .map_err(|err| {
                if let StorageWorkerJob::StorageProof { proof_result_sender, .. } = err.0 {
                    let _ = proof_result_sender.send(StorageProofResultMessage {
                        hashed_address,
                        result: Err(DatabaseError::Other(
                            "storage workers unavailable".to_string(),
                        )
                        .into()),
                    });
                }

                ProviderError::other(std::io::Error::other("storage workers unavailable"))
            })
    }

    /// Dispatch an account multiproof computation
    ///
    /// The result will be sent via the `result_sender` channel included in the input.
    pub fn dispatch_account_multiproof(
        &self,
        input: AccountMultiproofInput,
    ) -> Result<(), ProviderError> {
        self.account_work_tx
            .send(AccountWorkerJob::AccountMultiproof { input: Box::new(input) })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("account workers unavailable"));

                if let AccountWorkerJob::AccountMultiproof { input } = err.0 {
                    let ProofResultContext { sender: result_tx, state, start_time: start } =
                        input.into_proof_result_sender();

                    let _ = result_tx.send(ProofResultMessage {
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
        self.storage_work_tx
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
        self.account_work_tx
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
    fn compute_v2_storage_proof(
        &self,
        input: StorageProofInput,
        calculator: &mut proof_v2::StorageProofCalculator<
            <Provider as TrieCursorFactory>::StorageTrieCursor<'_>,
            <Provider as HashedCursorFactory>::StorageCursor<'_>,
        >,
    ) -> Result<StorageProofResult, StateProofError> {
        let StorageProofInput { hashed_address, mut targets } = input;

        let span = debug_span!(
            target: "trie::proof_task",
            "V2 Storage proof calculation",
            n = %targets.len(),
        );
        let _span_guard = span.enter();

        let proof_start = Instant::now();

        // If targets is empty it means the caller only wants the root node.
        let proof = if targets.is_empty() {
            let root_node = calculator.storage_root_node(hashed_address)?;
            vec![root_node]
        } else {
            calculator.storage_proof(hashed_address, &mut targets)?
        };

        let root = calculator.compute_root_hash(&proof)?;

        trace!(
            target: "trie::proof_task",
            hashed_address = ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            ?root,
            worker_id = self.id,
            "Completed V2 storage proof calculation"
        );

        Ok(StorageProofResult { proof, root })
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
    /// The proof calculation result
    pub result: Result<DecodedMultiProofV2, ParallelStateRootError>,
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
    /// Original state update that triggered this proof
    pub state: HashedPostState,
    /// Calculation start time for measuring elapsed duration
    pub start_time: Instant,
}

impl ProofResultContext {
    /// Creates a new proof result context.
    pub const fn new(
        sender: ProofResultSender,
        state: HashedPostState,
        start_time: Instant,
    ) -> Self {
        Self { sender, state, start_time }
    }
}

/// The results of a storage proof calculation.
#[derive(Debug)]
pub(crate) struct StorageProofResult {
    /// The calculated V2 proof nodes
    pub proof: Vec<ProofTrieNodeV2>,
    /// The storage root calculated by the V2 proof
    pub root: Option<B256>,
}

impl StorageProofResult {
    /// Returns the calculated root of the trie, if one can be calculated from the proof.
    const fn root(&self) -> Option<B256> {
        self.root
    }
}

/// Message containing a completed storage proof result with metadata.
#[derive(Debug)]
pub struct StorageProofResultMessage {
    /// The hashed address this storage proof belongs to
    #[allow(dead_code)]
    pub(crate) hashed_address: B256,
    /// The storage proof calculation result
    pub(crate) result: Result<StorageProofResult, StateProofError>,
}

/// Internal message for storage workers.
#[derive(Debug)]
pub(crate) enum StorageWorkerJob {
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
            worker_id = self.worker_id,
            "Storage worker started"
        );

        let mut storage_proofs_processed = 0u64;
        let mut storage_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();
        let trie_cursor = proof_tx.provider.storage_trie_cursor(B256::ZERO)?;
        let hashed_cursor = proof_tx.provider.hashed_storage_cursor(B256::ZERO)?;
        let mut v2_calculator =
            proof_v2::StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        // Initially mark this worker as available.
        self.available_workers.fetch_add(1, Ordering::Relaxed);

        let mut total_idle_time = Duration::ZERO;
        let mut idle_start = Instant::now();

        while let Ok(job) = self.work_rx.recv() {
            total_idle_time += idle_start.elapsed();

            // Mark worker as busy.
            self.available_workers.fetch_sub(1, Ordering::Relaxed);

            match job {
                StorageWorkerJob::StorageProof { input, proof_result_sender } => {
                    self.process_storage_proof(
                        &proof_tx,
                        &mut v2_calculator,
                        input,
                        proof_result_sender,
                        &mut storage_proofs_processed,
                    );
                }

                StorageWorkerJob::BlindedStorageNode { account, path, result_sender } => {
                    Self::process_blinded_node(
                        self.worker_id,
                        &proof_tx,
                        account,
                        path,
                        result_sender,
                        &mut storage_nodes_processed,
                    );
                }
            }

            // Mark worker as available again.
            self.available_workers.fetch_add(1, Ordering::Relaxed);

            idle_start = Instant::now();
        }

        trace!(
            target: "trie::proof_task",
            worker_id = self.worker_id,
            storage_proofs_processed,
            storage_nodes_processed,
            total_idle_time_us = total_idle_time.as_micros(),
            "Storage worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.record_storage_nodes(storage_nodes_processed as usize);
            self.metrics.record_storage_worker_idle_time(total_idle_time);
            self.cursor_metrics.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Processes a storage proof request.
    fn process_storage_proof<Provider>(
        &self,
        proof_tx: &ProofTaskTx<Provider>,
        v2_calculator: &mut proof_v2::StorageProofCalculator<
            <Provider as TrieCursorFactory>::StorageTrieCursor<'_>,
            <Provider as HashedCursorFactory>::StorageCursor<'_>,
        >,
        input: StorageProofInput,
        proof_result_sender: CrossbeamSender<StorageProofResultMessage>,
        storage_proofs_processed: &mut u64,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let hashed_address = input.hashed_address;
        let proof_start = Instant::now();

        trace!(
            target: "trie::proof_task",
            worker_id = self.worker_id,
            hashed_address = ?hashed_address,
            targets_len = input.targets.len(),
            "Processing V2 storage proof"
        );

        let result = proof_tx.compute_v2_storage_proof(input, v2_calculator);

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
            ?root,
            "Storage proof completed"
        );
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
    /// Channel for dispatching storage proof work (for pre-dispatched target proofs)
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
        let provider = self.task_ctx.factory.database_provider_ro()?;

        trace!(
            target: "trie::proof_task",
            worker_id=self.worker_id,
            "Account worker started"
        );

        let mut account_proofs_processed = 0u64;
        let mut account_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();

        // Create both account and storage calculators for V2 proofs.
        // The storage calculator is wrapped in Rc<RefCell<...>> for sharing with value encoders.
        let account_trie_cursor = provider.account_trie_cursor()?;
        let account_hashed_cursor = provider.hashed_account_cursor()?;

        let storage_trie_cursor = provider.storage_trie_cursor(B256::ZERO)?;
        let storage_hashed_cursor = provider.hashed_storage_cursor(B256::ZERO)?;

        let mut v2_account_calculator = proof_v2::ProofCalculator::<
            _,
            _,
            AsyncAccountValueEncoder<
                <Factory::Provider as TrieCursorFactory>::StorageTrieCursor<'_>,
                <Factory::Provider as HashedCursorFactory>::StorageCursor<'_>,
            >,
        >::new(account_trie_cursor, account_hashed_cursor);
        let v2_storage_calculator =
            Rc::new(RefCell::new(proof_v2::StorageProofCalculator::new_storage(
                storage_trie_cursor,
                storage_hashed_cursor,
            )));

        // Count this worker as available only after successful initialization.
        self.available_workers.fetch_add(1, Ordering::Relaxed);

        let mut total_idle_time = Duration::ZERO;
        let mut idle_start = Instant::now();
        let mut value_encoder_stats_cache = ValueEncoderStats::default();

        while let Ok(job) = self.work_rx.recv() {
            total_idle_time += idle_start.elapsed();

            // Mark worker as busy.
            self.available_workers.fetch_sub(1, Ordering::Relaxed);

            match job {
                AccountWorkerJob::AccountMultiproof { input } => {
                    let value_encoder_stats = self.process_account_multiproof::<Factory::Provider>(
                        &mut v2_account_calculator,
                        v2_storage_calculator.clone(),
                        *input,
                        &mut account_proofs_processed,
                        &mut cursor_metrics_cache,
                    );
                    total_idle_time += value_encoder_stats.storage_wait_time;
                    value_encoder_stats_cache.extend(&value_encoder_stats);
                }

                AccountWorkerJob::BlindedAccountNode { path, result_sender } => {
                    Self::process_blinded_node(
                        self.worker_id,
                        &provider,
                        path,
                        result_sender,
                        &mut account_nodes_processed,
                    );
                }
            }

            // Mark worker as available again.
            self.available_workers.fetch_add(1, Ordering::Relaxed);

            idle_start = Instant::now();
        }

        trace!(
            target: "trie::proof_task",
            worker_id=self.worker_id,
            account_proofs_processed,
            account_nodes_processed,
            total_idle_time_us = total_idle_time.as_micros(),
            "Account worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.record_account_nodes(account_nodes_processed as usize);
            self.metrics.record_account_worker_idle_time(total_idle_time);
            self.cursor_metrics.record(&mut cursor_metrics_cache);
            self.metrics.record_value_encoder_stats(&value_encoder_stats_cache);
        }

        Ok(())
    }

    fn compute_v2_account_multiproof<'a, Provider>(
        &self,
        v2_account_calculator: &mut V2AccountProofCalculator<'a, Provider>,
        v2_storage_calculator: Rc<RefCell<V2StorageProofCalculator<'a, Provider>>>,
        targets: MultiProofTargetsV2,
    ) -> Result<(DecodedMultiProofV2, ValueEncoderStats), ParallelStateRootError>
    where
        Provider: TrieCursorFactory + HashedCursorFactory + 'a,
    {
        let MultiProofTargetsV2 { mut account_targets, storage_targets } = targets;

        let span = debug_span!(
            target: "trie::proof_task",
            "Account V2 multiproof calculation",
            account_targets = account_targets.len(),
            storage_targets = storage_targets.values().map(|t| t.len()).sum::<usize>(),
        );
        let _span_guard = span.enter();

        trace!(target: "trie::proof_task", "Processing V2 account multiproof");

        let storage_proof_receivers =
            dispatch_v2_storage_proofs(&self.storage_work_tx, &account_targets, storage_targets)?;

        let mut value_encoder = AsyncAccountValueEncoder::new(
            storage_proof_receivers,
            self.cached_storage_roots.clone(),
            v2_storage_calculator,
        );

        let account_proofs =
            v2_account_calculator.proof(&mut value_encoder, &mut account_targets)?;

        let (storage_proofs, value_encoder_stats) = value_encoder.finalize()?;

        let proof = DecodedMultiProofV2 { account_proofs, storage_proofs };

        Ok((proof, value_encoder_stats))
    }

    /// Processes an account multiproof request.
    ///
    /// Returns stats from the value encoder used during proof computation.
    fn process_account_multiproof<'a, Provider>(
        &self,
        v2_account_calculator: &mut V2AccountProofCalculator<'a, Provider>,
        v2_storage_calculator: Rc<RefCell<V2StorageProofCalculator<'a, Provider>>>,
        input: AccountMultiproofInput,
        account_proofs_processed: &mut u64,
        cursor_metrics_cache: &mut ProofTaskCursorMetricsCache,
    ) -> ValueEncoderStats
    where
        Provider: TrieCursorFactory + HashedCursorFactory + 'a,
    {
        let proof_cursor_metrics = ProofTaskCursorMetricsCache::default();
        let proof_start = Instant::now();

        let AccountMultiproofInput { targets, proof_result_sender } = input;
        let (result, value_encoder_stats) = match self.compute_v2_account_multiproof::<Provider>(
            v2_account_calculator,
            v2_storage_calculator,
            targets,
        ) {
            Ok((proof, stats)) => (Ok(proof), stats),
            Err(e) => (Err(e), ValueEncoderStats::default()),
        };

        let ProofResultContext { sender: result_tx, state, start_time: start } =
            proof_result_sender;

        let proof_elapsed = proof_start.elapsed();
        let total_elapsed = start.elapsed();
        *account_proofs_processed += 1;

        // Send result to MultiProofTask
        if result_tx.send(ProofResultMessage { result, elapsed: total_elapsed, state }).is_err() {
            trace!(
                target: "trie::proof_task",
                worker_id=self.worker_id,
                account_proofs_processed,
                "Account multiproof receiver dropped, discarding result"
            );
        }

        proof_cursor_metrics.record_spans();

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

        value_encoder_stats
    }

    /// Processes a blinded account node lookup request.
    fn process_blinded_node<Provider>(
        worker_id: usize,
        provider: &Provider,
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
        );
        let _span_guard = span.enter();

        trace!(
            target: "trie::proof_task",
            "Processing blinded account node"
        );

        let start = Instant::now();
        let account_node_provider = ProofBlindedAccountProvider::new(provider, provider);
        let result = account_node_provider.trie_node(&path);
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

/// Queues V2 storage proofs for all accounts in the targets and returns receivers.
///
/// This function queues all storage proof tasks to the worker pool but returns immediately
/// with receivers, allowing the account trie walk to proceed in parallel with storage proof
/// computation. This enables interleaved parallelism for better performance.
///
/// Propagates errors up if queuing fails. Receivers must be consumed by the caller.
fn dispatch_v2_storage_proofs(
    storage_work_tx: &CrossbeamSender<StorageWorkerJob>,
    account_targets: &[proof_v2::Target],
    mut storage_targets: B256Map<Vec<proof_v2::Target>>,
) -> Result<B256Map<CrossbeamReceiver<StorageProofResultMessage>>, ParallelStateRootError> {
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(account_targets.len(), Default::default());

    // Collect hashed addresses from account targets that need their storage roots computed
    let account_target_addresses: B256Set = account_targets.iter().map(|t| t.key()).collect();

    // For storage targets with associated account proofs, ensure the first target has
    // min_len(0) so the root node is returned for storage root computation
    for (hashed_address, targets) in &mut storage_targets {
        if account_target_addresses.contains(hashed_address) &&
            let Some(first) = targets.first_mut()
        {
            *first = first.with_min_len(0);
        }
    }

    // Sort storage targets by address for optimal dispatch order.
    // Since trie walk processes accounts in lexicographical order, dispatching in the same order
    // reduces head-of-line blocking when consuming results.
    let mut sorted_storage_targets: Vec<_> = storage_targets.into_iter().collect();
    sorted_storage_targets.sort_unstable_by_key(|(addr, _)| *addr);

    // Dispatch all proofs for targeted storage slots
    for (hashed_address, targets) in sorted_storage_targets {
        // Create channel for receiving StorageProofResultMessage
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let input = StorageProofInput::new(hashed_address, targets);

        storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender: result_tx })
            .map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "Failed to queue storage proof for {hashed_address:?}: storage worker pool unavailable",
                ))
            })?;

        storage_proof_receivers.insert(hashed_address, result_rx);
    }

    Ok(storage_proof_receivers)
}

/// Input parameters for storage proof computation.
#[derive(Debug)]
pub struct StorageProofInput {
    /// The hashed address for which the proof is calculated.
    pub hashed_address: B256,
    /// The set of proof targets
    pub targets: Vec<proof_v2::Target>,
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given hashed address and target slots.
    pub const fn new(hashed_address: B256, targets: Vec<proof_v2::Target>) -> Self {
        Self { hashed_address, targets }
    }
}

/// Input parameters for account multiproof computation.
#[derive(Debug)]
pub struct AccountMultiproofInput {
    /// The targets for which to compute the multiproof.
    pub targets: MultiProofTargetsV2,
    /// Context for sending the proof result.
    pub proof_result_sender: ProofResultContext,
}

impl AccountMultiproofInput {
    /// Returns the [`ProofResultContext`] for this input, consuming the input.
    fn into_proof_result_sender(self) -> ProofResultContext {
        self.proof_result_sender
    }
}

/// Internal message for account workers.
#[derive(Debug)]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::test_utils::create_test_provider_factory;

    fn test_ctx<Factory>(factory: Factory) -> ProofTaskCtx<Factory> {
        ProofTaskCtx::new(factory)
    }

    /// Ensures `ProofWorkerHandle::new` spawns workers correctly.
    #[test]
    fn spawn_proof_workers_creates_handle() {
        let provider_factory = create_test_provider_factory();
        let changeset_cache = reth_trie_db::ChangesetCache::new();
        let factory = reth_provider::providers::OverlayStateProviderFactory::new(
            provider_factory,
            changeset_cache,
        );
        let ctx = test_ctx(factory);

        let runtime = reth_tasks::Runtime::test();
        let proof_handle = ProofWorkerHandle::new(&runtime, ctx, false);

        // Verify handle can be cloned
        let _cloned_handle = proof_handle.clone();

        // Workers shut down automatically when handle is dropped
        drop(proof_handle);
    }
}
