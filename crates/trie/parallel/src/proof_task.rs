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
    trie_cursor::{InstrumentedTrieCursor, TrieCursorFactory, TrieCursorMetricsCache},
    walker::TrieWalker,
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostState, MultiProofTargets,
    Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
    proof::{DecodedProofNodes, ProofRetainer},
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

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Batching configuration for storage proof job processing.
///
/// These constants control how storage workers collect and process jobs to improve
/// CPU cache utilization and reduce work queue polling overhead.
///
/// Note: This batches job *processing*, not result sending. Each proof result is still
/// sent individually through its dedicated channel. The performance benefit comes from:
/// - Reduced `recv()` syscalls on the work queue (using `try_recv()` after first `recv()`)
/// - Better CPU cache locality from processing similar jobs sequentially
/// - Reduced context switching overhead
mod batching {
    /// Maximum number of jobs to process together before checking for new work.
    ///
    /// Limits latency impact and ensures fairness among workers. Value of 32 balances
    /// throughput (processing similar jobs together) with responsiveness (checking for
    /// high-priority jobs frequently).
    pub(super) const MAX_BATCH_SIZE: usize = 32;

    /// Minimum queue depth required to trigger batch collection.
    ///
    /// When queue has fewer than 2 items, we process jobs individually to minimize
    /// latency. Batching only activates under load when there's actual work pressure.
    pub(super) const MIN_QUEUE_FOR_BATCHING: usize = 2;
}

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
    /// - `executor`: Tokio runtime handle for spawning blocking tasks
    /// - `task_ctx`: Shared context with database view and prefix sets
    /// - `storage_worker_count`: Number of storage workers to spawn
    /// - `account_worker_count`: Number of account workers to spawn
    pub fn new<Factory>(
        executor: Handle,
        task_ctx: ProofTaskCtx<Factory>,
        storage_worker_count: usize,
        account_worker_count: usize,
    ) -> Self
    where
        Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + 'static,
    {
        let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();
        let (account_work_tx, account_work_rx) = unbounded::<AccountWorkerJob>();

        // Initialize availability counters at zero. Each worker will increment when it
        // successfully initializes, ensuring only healthy workers are counted.
        let storage_available_workers = Arc::new(AtomicUsize::new(0));
        let account_available_workers = Arc::new(AtomicUsize::new(0));

        debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            "Spawning proof worker pools"
        );

        let parent_span =
            debug_span!(target: "trie::proof_task", "storage proof workers", ?storage_worker_count)
                .entered();
        // Spawn storage workers
        for worker_id in 0..storage_worker_count {
            let span = debug_span!(target: "trie::proof_task", "storage worker", ?worker_id);
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = storage_work_rx.clone();
            let storage_available_workers_clone = storage_available_workers.clone();

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
        }
        drop(parent_span);

        let parent_span =
            debug_span!(target: "trie::proof_task", "account proof workers", ?account_worker_count)
                .entered();
        // Spawn account workers
        for worker_id in 0..account_worker_count {
            let span = debug_span!(target: "trie::proof_task", "account worker", ?worker_id);
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = account_work_rx.clone();
            let storage_work_tx_clone = storage_work_tx.clone();
            let account_available_workers_clone = account_available_workers.clone();

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
        }
        drop(parent_span);

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
        proof_result_sender: ProofResultContext,
    ) -> Result<(), ProviderError> {
        self.storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("storage workers unavailable"));

                if let StorageWorkerJob::StorageProof { proof_result_sender, .. } = err.0 {
                    let ProofResultContext {
                        sender: result_tx,
                        sequence_number: seq,
                        state,
                        start_time: start,
                    } = proof_result_sender;

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

/// Data used for initializing cursor factories that is shared across all storage proof instances.
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

/// This contains all information shared between all storage proof instances.
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
    fn compute_storage_proof(
        &self,
        input: StorageProofInput,
        trie_cursor_metrics: &mut TrieCursorMetricsCache,
        hashed_cursor_metrics: &mut HashedCursorMetricsCache,
    ) -> StorageProofResult {
        // Consume the input so we can move large collections (e.g. target slots) without cloning.
        let StorageProofInput {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        } = input;

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

        // Decode proof into DecodedStorageMultiProof
        let decoded_result = raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    hashed_address, e
                ))
            })
        });

        trace!(
            target: "trie::proof_task",
            hashed_address = ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            worker_id = self.id,
            "Completed storage proof calculation"
        );

        decoded_result
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
/// Result of a proof calculation, which can be either an account multiproof or a storage proof.
#[derive(Debug)]
pub enum ProofResult {
    /// Account multiproof with statistics
    AccountMultiproof {
        /// The account multiproof
        proof: DecodedMultiProof,
        /// Statistics collected during proof computation
        stats: ParallelTrieStats,
    },
    /// Storage proof for a specific account
    StorageProof {
        /// The hashed address this storage proof belongs to
        hashed_address: B256,
        /// The storage multiproof
        proof: DecodedStorageMultiProof,
    },
}

impl ProofResult {
    /// Convert this proof result into a `DecodedMultiProof`.
    ///
    /// For account multiproofs, returns the multiproof directly (discarding stats).
    /// For storage proofs, wraps the storage proof into a minimal multiproof.
    pub fn into_multiproof(self) -> DecodedMultiProof {
        match self {
            Self::AccountMultiproof { proof, stats: _ } => proof,
            Self::StorageProof { hashed_address, proof } => {
                DecodedMultiProof::from_storage_proof(hashed_address, proof)
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
/// Internal message for storage workers.
#[derive(Debug)]
enum StorageWorkerJob {
    /// Storage proof computation request
    StorageProof {
        /// Storage proof input parameters
        input: StorageProofInput,
        /// Context for sending the proof result.
        proof_result_sender: ProofResultContext,
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
        #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
        #[cfg(feature = "metrics")] cursor_metrics: ProofTaskCursorMetrics,
    ) -> Self {
        Self {
            task_ctx,
            work_rx,
            worker_id,
            available_workers,
            #[cfg(feature = "metrics")]
            metrics,
            #[cfg(feature = "metrics")]
            cursor_metrics,
        }
    }

    /// Attempts to collect a batch of jobs from the work queue for processing.
    ///
    /// Adaptively determines batch size based on current queue depth:
    /// - Queue depth < 2: Return single job (minimize latency)
    /// - Queue depth 2-32: Collect available jobs up to queue depth
    /// - Queue depth > 32: Collect up to `MAX_BATCH_SIZE` (limit latency impact)
    ///
    /// All job types (storage proofs and blinded node requests) are included in the
    /// returned batch. Caller is responsible for separating and processing appropriately.
    ///
    /// Stops collecting early if a blinded node request is encountered, as these are
    /// latency-sensitive and should be processed quickly.
    fn try_collect_batch_static(
        work_rx: &CrossbeamReceiver<StorageWorkerJob>,
        worker_id: usize,
        first_job: StorageWorkerJob,
    ) -> Vec<StorageWorkerJob> {
        // Blinded node requests are latency-sensitive, never batch them
        if matches!(first_job, StorageWorkerJob::BlindedStorageNode { .. }) {
            return vec![first_job];
        }

        let queue_len = work_rx.len();

        // If queue is small, process single job to minimize latency
        if queue_len < batching::MIN_QUEUE_FOR_BATCHING {
            return vec![first_job];
        }

        // Determine target batch size based on queue pressure
        let target_batch_size = queue_len.min(batching::MAX_BATCH_SIZE);

        let mut batch = Vec::with_capacity(target_batch_size);
        batch.push(first_job);

        // Collect additional storage proof jobs (non-blocking)
        for _ in 1..target_batch_size {
            match work_rx.try_recv() {
                Ok(job) => {
                    // Check if this is a blinded node request before moving the job
                    let is_blinded_node = matches!(job, StorageWorkerJob::BlindedStorageNode { .. });

                    // Include all job types in batch - they'll be separated during processing
                    batch.push(job);

                    // Stop collecting if we encountered a blinded node request
                    // (it needs immediate processing for low latency)
                    if is_blinded_node {
                        break;
                    }
                }
                Err(
                    crossbeam_channel::TryRecvError::Empty |
                    crossbeam_channel::TryRecvError::Disconnected,
                ) => break,
            }
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            batch_size = batch.len(),
            queue_len,
            "Collected storage proof batch"
        );

        batch
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
        let Self {
            task_ctx,
            work_rx,
            worker_id,
            available_workers,
            #[cfg(feature = "metrics")]
            metrics,
            #[cfg(feature = "metrics")]
            ref mut cursor_metrics,
        } = self;

        // Create provider from factory
        let provider = task_ctx.factory.database_provider_ro()?;
        let proof_tx = ProofTaskTx::new(provider, worker_id);

        trace!(
            target: "trie::proof_task",
            worker_id,
            "Storage worker started"
        );

        let mut storage_proofs_processed = 0u64;
        let mut storage_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();

        // Initially mark this worker as available.
        available_workers.fetch_add(1, Ordering::Relaxed);

        while let Ok(first_job) = work_rx.recv() {
            // Mark worker as busy.
            available_workers.fetch_sub(1, Ordering::Relaxed);

            // Try to collect a batch of jobs for processing
            let jobs = Self::try_collect_batch_static(&work_rx, worker_id, first_job);

            // Separate job types for appropriate handling
            let mut storage_proofs = Vec::new();
            let mut blinded_nodes = Vec::new();

            for job in jobs {
                match job {
                    StorageWorkerJob::StorageProof { .. } => storage_proofs.push(job),
                    StorageWorkerJob::BlindedStorageNode { .. } => blinded_nodes.push(job),
                }
            }

            // Process storage proofs (batched if multiple, individual if single)
            let storage_proof_count = storage_proofs.len();
            if storage_proof_count > 1 {
                // Process as batch for better cache utilization
                Self::process_storage_proof_batch(
                    worker_id,
                    &proof_tx,
                    storage_proofs,
                    &mut storage_proofs_processed,
                    &mut cursor_metrics_cache,
                );
            } else {
                // Process individually
                for job in storage_proofs {
                    if let StorageWorkerJob::StorageProof { input, proof_result_sender } = job {
                        Self::process_storage_proof(
                            worker_id,
                            &proof_tx,
                            input,
                            proof_result_sender,
                            &mut storage_proofs_processed,
                            &mut cursor_metrics_cache,
                        );
                    }
                }
            }

            // Always process blinded node requests individually (latency-sensitive)
            for job in blinded_nodes {
                if let StorageWorkerJob::BlindedStorageNode { account, path, result_sender } = job {
                    Self::process_blinded_node(
                        worker_id,
                        &proof_tx,
                        account,
                        path,
                        result_sender,
                        &mut storage_nodes_processed,
                    );
                }
            }

            // Mark worker as available again.
            available_workers.fetch_add(1, Ordering::Relaxed);
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            storage_proofs_processed,
            storage_nodes_processed,
            "Storage worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            metrics.record_storage_nodes(storage_nodes_processed as usize);
            cursor_metrics.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Processes a storage proof request.
    fn process_storage_proof<Provider>(
        worker_id: usize,
        proof_tx: &ProofTaskTx<Provider>,
        input: StorageProofInput,
        proof_result_sender: ProofResultContext,
        storage_proofs_processed: &mut u64,
        cursor_metrics_cache: &mut ProofTaskCursorMetricsCache,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let hashed_address = input.hashed_address;
        let ProofResultContext { sender, sequence_number: seq, state, start_time } =
            proof_result_sender;

        let mut trie_cursor_metrics = TrieCursorMetricsCache::default();
        let mut hashed_cursor_metrics = HashedCursorMetricsCache::default();

        trace!(
            target: "trie::proof_task",
            worker_id,
            hashed_address = ?hashed_address,
            prefix_set_len = input.prefix_set.len(),
            target_slots_len = input.target_slots.len(),
            "Processing storage proof"
        );

        let proof_start = Instant::now();
        let result = proof_tx.compute_storage_proof(
            input,
            &mut trie_cursor_metrics,
            &mut hashed_cursor_metrics,
        );

        let proof_elapsed = proof_start.elapsed();
        *storage_proofs_processed += 1;

        let result_msg = result.map(|storage_proof| ProofResult::StorageProof {
            hashed_address,
            proof: storage_proof,
        });

        if sender
            .send(ProofResultMessage {
                sequence_number: seq,
                result: result_msg,
                elapsed: start_time.elapsed(),
                state,
            })
            .is_err()
        {
            trace!(
                target: "trie::proof_task",
                worker_id,
                hashed_address = ?hashed_address,
                storage_proofs_processed,
                "Proof result receiver dropped, discarding result"
            );
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            hashed_address = ?hashed_address,
            proof_time_us = proof_elapsed.as_micros(),
            total_processed = storage_proofs_processed,
            trie_cursor_duration_us = trie_cursor_metrics.total_duration.as_micros(),
            hashed_cursor_duration_us = hashed_cursor_metrics.total_duration.as_micros(),
            ?trie_cursor_metrics,
            ?hashed_cursor_metrics,
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

    /// Processes a batch of storage proof requests for better CPU cache utilization.
    ///
    /// Processing similar jobs sequentially improves cache locality and reduces overhead
    /// from context switching. Each proof is still computed and sent individually.
    ///
    /// Performance benefits:
    /// - Better instruction cache utilization (same code path executed repeatedly)
    /// - Better data cache utilization (similar data structures accessed)
    /// - Reduced overhead from repeatedly checking the work queue
    fn process_storage_proof_batch<Provider>(
        worker_id: usize,
        proof_tx: &ProofTaskTx<Provider>,
        jobs: Vec<StorageWorkerJob>,
        storage_proofs_processed: &mut u64,
        cursor_metrics_cache: &mut ProofTaskCursorMetricsCache,
    ) where
        Provider: TrieCursorFactory + HashedCursorFactory,
    {
        let batch_start = Instant::now();
        let batch_size = jobs.len();

        trace!(
            target: "trie::proof_task",
            worker_id,
            batch_size,
            "Processing storage proof batch"
        );

        // Process each proof in the batch
        for job in jobs {
            if let StorageWorkerJob::StorageProof { input, proof_result_sender } = job {
                Self::process_storage_proof(
                    worker_id,
                    proof_tx,
                    input,
                    proof_result_sender,
                    storage_proofs_processed,
                    cursor_metrics_cache,
                );
            }
        }

        let batch_elapsed = batch_start.elapsed();

        trace!(
            target: "trie::proof_task",
            worker_id,
            batch_size,
            batch_elapsed_us = batch_elapsed.as_micros(),
            avg_proof_time_us = batch_elapsed.as_micros() / batch_size as u128,
            "Completed storage proof batch"
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
    /// Channel for dispatching storage proof work
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Counter tracking worker availability
    available_workers: Arc<AtomicUsize>,
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
    const fn new(
        task_ctx: ProofTaskCtx<Factory>,
        work_rx: CrossbeamReceiver<AccountWorkerJob>,
        worker_id: usize,
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        available_workers: Arc<AtomicUsize>,
        #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
        #[cfg(feature = "metrics")] cursor_metrics: ProofTaskCursorMetrics,
    ) -> Self {
        Self {
            task_ctx,
            work_rx,
            worker_id,
            storage_work_tx,
            available_workers,
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
        let Self {
            task_ctx,
            work_rx,
            worker_id,
            storage_work_tx,
            available_workers,
            #[cfg(feature = "metrics")]
            metrics,
            #[cfg(feature = "metrics")]
            ref mut cursor_metrics,
        } = self;

        // Create provider from factory
        let provider = task_ctx.factory.database_provider_ro()?;
        let proof_tx = ProofTaskTx::new(provider, worker_id);

        trace!(
            target: "trie::proof_task",
            worker_id,
            "Account worker started"
        );

        let mut account_proofs_processed = 0u64;
        let mut account_nodes_processed = 0u64;
        let mut cursor_metrics_cache = ProofTaskCursorMetricsCache::default();

        // Count this worker as available only after successful initialization.
        available_workers.fetch_add(1, Ordering::Relaxed);

        while let Ok(job) = work_rx.recv() {
            // Mark worker as busy.
            available_workers.fetch_sub(1, Ordering::Relaxed);

            match job {
                AccountWorkerJob::AccountMultiproof { input } => {
                    Self::process_account_multiproof(
                        worker_id,
                        &proof_tx,
                        storage_work_tx.clone(),
                        *input,
                        &mut account_proofs_processed,
                        &mut cursor_metrics_cache,
                    );
                }

                AccountWorkerJob::BlindedAccountNode { path, result_sender } => {
                    Self::process_blinded_node(
                        worker_id,
                        &proof_tx,
                        path,
                        result_sender,
                        &mut account_nodes_processed,
                    );
                }
            }

            // Mark worker as available again.
            available_workers.fetch_add(1, Ordering::Relaxed);
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            account_proofs_processed,
            account_nodes_processed,
            "Account worker shutting down"
        );

        #[cfg(feature = "metrics")]
        {
            metrics.record_account_nodes(account_nodes_processed as usize);
            cursor_metrics.record(&mut cursor_metrics_cache);
        }

        Ok(())
    }

    /// Processes an account multiproof request.
    fn process_account_multiproof<Provider>(
        worker_id: usize,
        proof_tx: &ProofTaskTx<Provider>,
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
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
            missed_leaves_storage_roots,
            proof_result_sender:
                ProofResultContext { sender: result_tx, sequence_number: seq, state, start_time: start },
        } = input;

        let span = debug_span!(
            target: "trie::proof_task",
            "Account multiproof calculation",
            targets = targets.len(),
            worker_id,
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
            &storage_work_tx,
            &targets,
            &mut storage_prefix_sets,
            collect_branch_node_masks,
            multi_added_removed_keys.as_ref(),
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
            missed_leaves_storage_roots: missed_leaves_storage_roots.as_ref(),
        };

        let result =
            build_account_multiproof_with_storage_roots(&proof_tx.provider, ctx, &mut tracker);

        let proof_elapsed = proof_start.elapsed();
        let total_elapsed = start.elapsed();
        let proof_cursor_metrics = tracker.cursor_metrics;

        let stats = tracker.finish();
        let result = result.map(|proof| ProofResult::AccountMultiproof { proof, stats });
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
                worker_id,
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

                        // Extract storage proof from the result
                        let proof = match proof_msg.result? {
                            ProofResult::StorageProof { hashed_address: addr, proof } => {
                                debug_assert_eq!(
                                    addr,
                                    hashed_address,
                                    "storage worker must return same address: expected {hashed_address}, got {addr}"
                                );
                                proof
                            }
                            ProofResult::AccountMultiproof { .. } => {
                                unreachable!("storage worker only sends StorageProof variant")
                            }
                        };

                        let root = proof.root;
                        collected_decoded_storages.insert(hashed_address, proof);
                        root
                    }
                    // Since we do not store all intermediate nodes in the database, there might
                    // be a possibility of re-adding a non-modified leaf to the hash builder.
                    None => {
                        tracker.inc_missed_leaves();

                        match ctx.missed_leaves_storage_roots.entry(hashed_address) {
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

    // Consume remaining storage proof receivers for accounts not encountered during trie walk.
    for (hashed_address, receiver) in storage_proof_receivers {
        if let Ok(proof_msg) = receiver.recv() {
            // Extract storage proof from the result
            if let Ok(ProofResult::StorageProof { proof, .. }) = proof_msg.result {
                collected_decoded_storages.insert(hashed_address, proof);
            }
        }
    }

    let _ = hash_builder.root();

    let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
    let decoded_account_subtree = DecodedProofNodes::try_from(account_subtree_raw_nodes)?;

    let (branch_node_hash_masks, branch_node_tree_masks) = if ctx.collect_branch_node_masks {
        let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
        (
            updated_branch_nodes.iter().map(|(path, node)| (*path, node.hash_mask)).collect(),
            updated_branch_nodes.into_iter().map(|(path, node)| (path, node.tree_mask)).collect(),
        )
    } else {
        (Default::default(), Default::default())
    };

    // Extend tracker with accumulated metrics from account cursors
    tracker.cursor_metrics.account_trie_cursor.extend(&account_trie_cursor_metrics);
    tracker.cursor_metrics.account_hashed_cursor.extend(&account_hashed_cursor_metrics);

    Ok(DecodedMultiProof {
        account_subtree: decoded_account_subtree,
        branch_node_hash_masks,
        branch_node_tree_masks,
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
) -> Result<B256Map<CrossbeamReceiver<ProofResultMessage>>, ParallelStateRootError> {
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());

    // Dispatch all storage proofs to worker pool
    for (hashed_address, target_slots) in targets.iter() {
        let prefix_set = storage_prefix_sets.remove(hashed_address).unwrap_or_default();

        // Create channel for receiving ProofResultMessage
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let start = Instant::now();

        // Create computation input (data only, no communication channel)
        let input = StorageProofInput::new(
            *hashed_address,
            prefix_set,
            target_slots.clone(),
            with_branch_node_masks,
            multi_added_removed_keys.cloned(),
        );

        // Always dispatch a storage proof so we obtain the storage root even when no slots are
        // requested.
        storage_work_tx
            .send(StorageWorkerJob::StorageProof {
                input,
                proof_result_sender: ProofResultContext::new(
                    result_tx,
                    0,
                    HashedPostState::default(),
                    start,
                ),
            })
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
pub struct StorageProofInput {
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
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given hashed address, prefix set, and target
    /// slots.
    pub const fn new(
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: B256Set,
        with_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    ) -> Self {
        Self {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
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
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    pub missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
    /// Context for sending the proof result.
    pub proof_result_sender: ProofResultContext,
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
    storage_proof_receivers: B256Map<CrossbeamReceiver<ProofResultMessage>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    missed_leaves_storage_roots: &'a DashMap<B256, B256>,
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
            let factory =
                reth_provider::providers::OverlayStateProviderFactory::new(provider_factory);
            let ctx = test_ctx(factory);

            let proof_handle = ProofWorkerHandle::new(handle.clone(), ctx, 5, 3);

            // Verify handle can be cloned
            let _cloned_handle = proof_handle.clone();

            // Workers shut down automatically when handle is dropped
            drop(proof_handle);
            task::yield_now().await;
        });
    }
}
