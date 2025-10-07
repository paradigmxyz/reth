//! A Task that manages sending proof requests to a number of tasks that have longer-running
//! database transactions.
//!
//! The [`ProofTaskManager`] ensures that there are a max number of currently executing proof tasks,
//! and is responsible for managing the fixed number of database transactions created at the start
//! of the task.
//!
//! Individual [`ProofTaskTx`] instances manage a dedicated [`InMemoryTrieCursorFactory`] and
//! [`HashedPostStateCursorFactory`], which are each backed by a database transaction.

use crate::root::ParallelStateRootError;
use alloy_primitives::{map::B256Set, B256};
use crossbeam_channel::{bounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::TriePrefixSetsMut,
    proof::{ProofTrieNodeProviderFactory, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    DecodedStorageMultiProof, HashedPostStateSorted, Nibbles,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_sparse::provider::{RevealedNode, TrieNodeProvider, TrieNodeProviderFactory};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, SendError, Sender},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::{debug, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Internal message for storage proof workers.
///
/// This is NOT exposed publicly. External callers still use `ProofTaskKind::StorageProof`
/// which is routed through the manager's `std::mpsc` channel.
#[derive(Debug)]
struct StorageProofJob {
    /// Storage proof input parameters
    input: StorageProofInput,
    /// Channel to send result back to original caller
    ///
    /// This is the same `std::mpsc::Sender` that the external caller provided in
    /// `ProofTaskKind::StorageProof(input`, sender).
    result_sender: Sender<StorageProofResult>,
}

/// Internal message for on-demand task execution.
///
/// These tasks are executed with lazily-created transactions that are
/// returned to the pool after use (same as current behavior).
#[derive(Debug)]
#[allow(dead_code)]
enum OnDemandTask {
    /// Fetch a blinded account node by path
    BlindedAccountNode(Nibbles, Sender<TrieNodeProviderResult>),
    /// Fetch a blinded storage node by account and path
    BlindedStorageNode(B256, Nibbles, Sender<TrieNodeProviderResult>),
}

/// A task that manages sending proof requests to worker pools and on-demand tasks.
///
/// # Architecture (PR1: Storage Workers Only)
///
/// This manager maintains two execution paths:
///
/// 1. **Storage Worker Pool** (NEW):
///    - Pre-spawned workers with dedicated long-lived transactions
///    - Tasks queued via crossbeam bounded channel
///    - Workers continuously process without transaction return overhead
///
/// 2. **On-Demand Execution** (EXISTING):
///    - Lazy transaction creation for blinded node fetches
///    - Transactions returned to pool after use (original behavior)
///    - Same message-passing mechanism as before
///
/// # External API
///
/// The external API via `ProofTaskManagerHandle` is COMPLETELY UNCHANGED:
/// - `queue_task(ProofTaskKind)` signature identical
/// - Same `std::mpsc` message passing
/// - Same return types and error handling
///
/// All changes are internal routing optimizations.
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    // ==================== STORAGE WORKER POOL (NEW) ====================
    /// Sender for storage proof tasks to worker pool.
    ///
    /// Queue capacity = `storage_worker_count` * 2 (for 2x buffering)
    storage_work_tx: CrossbeamSender<StorageProofJob>,

    /// Number of storage workers successfully spawned.
    ///
    /// May be less than requested if transaction creation fails.
    storage_worker_count: usize,

    // ==================== ON-DEMAND TRANSACTION POOL (REFACTORED) ====================
    /// Maximum number of on-demand transactions for blinded node fetches.
    ///
    /// Calculated as: `max_concurrency` - `storage_worker_count`
    max_on_demand_txs: usize,

    /// Currently available on-demand transactions (reused after return).
    ///
    /// Same lifecycle as before PR1.
    on_demand_txs: Vec<ProofTaskTx<FactoryTx<Factory>>>,

    /// Total on-demand transactions created (for ID assignment).
    on_demand_tx_count: usize,

    /// Queue of pending on-demand tasks waiting for available transaction.
    ///
    /// Replaces the old `pending_tasks` `VecDeque` which held all task types.
    /// TODO: Change to `VecDeque`<OnDemandTask> in Phase 8 when implementing proper task routing
    pending_on_demand: VecDeque<ProofTaskKind>,

    // ==================== SHARED RESOURCES  ====================
    /// Consistent view provider used for creating transactions on-demand.
    view: ConsistentDbView<Factory>,

    /// Proof task context shared across all proof tasks.
    task_ctx: ProofTaskCtx,

    /// The underlying handle from which to spawn proof tasks.
    executor: Handle,

    /// A receiver for new proof task messages from external callers.
    ///
    /// This is the `std::mpsc` channel connected to [`ProofTaskManagerHandle`].
    /// UNCHANGED - maintains interface compatibility.
    proof_task_rx: Receiver<ProofTaskMessage<FactoryTx<Factory>>>,

    /// A sender for internal messaging (transaction returns).
    ///
    /// Used by on-demand tasks to return transactions to pool.
    tx_sender: Sender<ProofTaskMessage<FactoryTx<Factory>>>,

    /// The number of active handles.
    ///
    /// Incremented in [`ProofTaskManagerHandle::new`] and decremented in
    /// [`ProofTaskManagerHandle::drop`].
    active_handles: Arc<AtomicUsize>,

    /// Metrics tracking proof task operations.
    #[cfg(feature = "metrics")]
    metrics: ProofTaskMetrics,
}

/// Worker loop for storage proof computation.
///
/// # Lifecycle
///
/// Each worker:
/// 1. Receives `StorageProofJob` from crossbeam bounded channel
/// 2. Computes proof using its dedicated long-lived transaction
/// 3. Sends result directly to original caller via `std::mpsc`
/// 4. Repeats until channel closes (graceful shutdown)
///
/// # Transaction Reuse
///
/// The key optimization: the worker reuses the same `proof_tx` across ALL proofs,
/// avoiding the overhead of:
/// - Creating new database transactions
/// - Setting up cursor factories
/// - Returning transactions to a pool
///
/// # Panic Safety
///
/// If this function panics, the worker thread terminates but:
/// - Other workers continue operating
/// - The manager detects disconnection when trying to send
/// - System degrades gracefully rather than failing completely
///
/// # Shutdown
///
/// Worker shuts down when:
/// - Crossbeam channel closes (all senders dropped)
/// - `ProofTaskManager::run()` drops `storage_work_tx` on terminate
fn storage_worker_loop<Tx>(
    proof_tx: ProofTaskTx<Tx>,
    work_rx: CrossbeamReceiver<StorageProofJob>,
    worker_id: usize,
) where
    Tx: DbTx,
{
    tracing::debug!(
        target: "trie::proof_task",
        worker_id,
        "Storage proof worker started"
    );

    let mut proofs_processed = 0u64;
    let start_time = Instant::now();

    // Main worker loop: process jobs until channel closes
    while let Ok(StorageProofJob { input, result_sender }) = work_rx.recv() {
        let proof_start = Instant::now();

        trace!(
            target: "trie::proof_task",
            worker_id,
            hashed_address = ?input.hashed_address,
            prefix_set_len = input.prefix_set.len(),
            target_slots = input.target_slots.len(),
            "Processing storage proof"
        );

        // ==================== CORE COMPUTATION ====================
        // Compute storage proof using reused transaction
        // This is the key difference from on-demand execution:
        // - No transaction creation overhead
        // - No transaction return message
        // - Cursor factories reused across proofs
        let result = proof_tx.compute_storage_proof(&input);

        let proof_elapsed = proof_start.elapsed();
        proofs_processed += 1;

        // ==================== RESULT DELIVERY ====================
        // Send result directly to original caller's std::mpsc::Receiver
        // If receiver is dropped (caller cancelled), log and continue
        if result_sender.send(result).is_err() {
            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                hashed_address = ?input.hashed_address,
                proofs_processed,
                "Storage proof receiver dropped, discarding result"
            );
        }

        trace!(
            target: "trie::proof_task",
            worker_id,
            hashed_address = ?input.hashed_address,
            proof_time_us = proof_elapsed.as_micros(),
            total_processed = proofs_processed,
            "Storage proof completed"
        );
    }

    // Channel closed - graceful shutdown
    let total_elapsed = start_time.elapsed();

    tracing::info!(
        target: "trie::proof_task",
        worker_id,
        proofs_processed,
        uptime_secs = total_elapsed.as_secs(),
        avg_proof_time_ms = if proofs_processed > 0 {
            total_elapsed.as_millis() / proofs_processed as u128
        } else {
            0
        },
        "Storage proof worker shutting down"
    );
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    /// Creates a new [`ProofTaskManager`] with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `executor` - Tokio runtime handle for spawning workers and tasks
    /// * `view` - Consistent database view for creating read-only transactions
    /// * `task_ctx` - Shared context (trie updates, hashed state, prefix sets)
    /// * `max_concurrency` - Total transaction budget across all execution paths
    /// * `storage_worker_count` - Number of storage proof workers to pre-spawn
    ///
    /// # Transaction Budget Allocation
    ///
    /// The total `max_concurrency` is split between two pools:
    ///
    /// 1. **Storage Workers**: `storage_worker_count` transactions (pre-allocated)
    /// 2. **On-Demand Pool**: `max_concurrency - storage_worker_count` (lazy)
    ///
    /// Example:
    /// ```text
    /// max_concurrency = 8, storage_worker_count = 4
    /// → 4 storage workers (pre-spawned)
    /// → 4 on-demand transactions (created lazily for blinded nodes)
    /// Total: 8 transactions max (same capacity as before)
    /// ```
    ///
    /// # Worker Spawn Resilience
    ///
    /// If some workers fail to spawn (e.g., transaction creation error):
    /// - Failed workers are logged and skipped
    /// - On-demand pool is adjusted: `max_concurrency - actual_spawned_workers`
    /// - System continues with fewer workers rather than failing entirely
    ///
    /// # Panics
    ///
    /// Does not panic. All errors are logged and handled gracefully.
    pub fn new(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        max_concurrency: usize,
        storage_worker_count: usize,
    ) -> Self {
        // Create message channel for external callers (UNCHANGED)
        let (tx_sender, proof_task_rx) = channel();

        // ==================== STORAGE WORKER POOL SETUP ====================

        // Queue capacity: 2x buffering to reduce contention
        // If workers = 4, queue holds 8 tasks maximum
        let queue_capacity = storage_worker_count.saturating_mul(2).max(1);
        let (storage_work_tx, storage_work_rx) = bounded::<StorageProofJob>(queue_capacity);

        tracing::info!(
            target: "trie::proof_task",
            storage_worker_count,
            queue_capacity,
            max_concurrency,
            "Initializing storage proof worker pool"
        );

        // Spawn storage workers - each gets its own long-lived transaction
        let mut spawned_workers = 0;
        for worker_id in 0..storage_worker_count {
            // Try to create transaction for this worker
            match view.provider_ro() {
                Ok(provider_ro) => {
                    let tx = provider_ro.into_tx();
                    let proof_task_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id);
                    let work_rx = storage_work_rx.clone();

                    // Spawn worker on tokio blocking pool
                    executor.spawn_blocking(move || {
                        storage_worker_loop(proof_task_tx, work_rx, worker_id)
                    });

                    spawned_workers += 1;

                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        spawned_workers,
                        "Storage worker spawned successfully"
                    );
                }
                Err(err) => {
                    // Non-fatal: log and continue with fewer workers
                    tracing::warn!(
                        target: "trie::proof_task",
                        worker_id,
                        ?err,
                        requested = storage_worker_count,
                        spawned_workers,
                        "Failed to create transaction for storage worker, continuing with fewer workers"
                    );
                }
            }
        }

        // Verify we spawned at least some workers
        if spawned_workers == 0 {
            tracing::error!(
                target: "trie::proof_task",
                requested = storage_worker_count,
                "Failed to spawn any storage workers - all will use on-demand pool"
            );
        } else if spawned_workers < storage_worker_count {
            tracing::warn!(
                target: "trie::proof_task",
                requested = storage_worker_count,
                spawned = spawned_workers,
                "Spawned fewer storage workers than requested"
            );
        } else {
            tracing::info!(
                target: "trie::proof_task",
                spawned_workers,
                queue_capacity,
                "Storage worker pool initialized successfully"
            );
        }

        // ==================== ON-DEMAND POOL SETUP ====================

        // Calculate on-demand budget: remaining capacity after storage workers
        // Ensure at least 1 on-demand transaction even if storage workers consume all budget
        let max_on_demand_txs = max_concurrency.saturating_sub(spawned_workers).max(1);

        tracing::debug!(
            target: "trie::proof_task",
            max_on_demand_txs,
            storage_workers = spawned_workers,
            total_capacity = max_concurrency,
            "Configured on-demand transaction pool for blinded nodes"
        );

        // ==================== CONSTRUCT MANAGER ====================

        Self {
            // Storage worker pool
            storage_work_tx,
            storage_worker_count: spawned_workers,

            // On-demand pool
            max_on_demand_txs,
            on_demand_txs: Vec::with_capacity(max_on_demand_txs),
            on_demand_tx_count: 0,
            pending_on_demand: VecDeque::new(),

            // Shared resources
            view,
            task_ctx,
            executor,
            proof_task_rx,
            tx_sender,
            active_handles: Arc::new(AtomicUsize::new(0)),

            #[cfg(feature = "metrics")]
            metrics: ProofTaskMetrics::default(),
        }
    }

    /// Returns a handle for sending new proof tasks to the manager.
    ///
    /// # Interface Compatibility
    ///
    /// This method is UNCHANGED from the original implementation. The returned
    /// `ProofTaskManagerHandle` has the exact same public API as before PR1.
    pub fn handle(&self) -> ProofTaskManagerHandle<FactoryTx<Factory>> {
        ProofTaskManagerHandle::new(self.tx_sender.clone(), self.active_handles.clone())
    }
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + 'static,
{
    /// Inserts the task into the pending tasks queue.
    pub fn queue_proof_task(&mut self, task: ProofTaskKind) {
        self.pending_on_demand.push_back(task);
    }

    /// Gets either the next available transaction, or creates a new one if all are in use and the
    /// total number of transactions created is less than the max concurrency.
    pub fn get_or_create_tx(&mut self) -> ProviderResult<Option<ProofTaskTx<FactoryTx<Factory>>>> {
        if let Some(proof_task_tx) = self.on_demand_txs.pop() {
            return Ok(Some(proof_task_tx));
        }

        // if we can create a new tx within our concurrency limits, create one on-demand
        if self.on_demand_tx_count < self.max_on_demand_txs {
            let provider_ro = self.view.provider_ro()?;
            let tx = provider_ro.into_tx();
            self.on_demand_tx_count += 1;
            return Ok(Some(ProofTaskTx::new(tx, self.task_ctx.clone(), self.on_demand_tx_count)));
        }

        Ok(None)
    }

    /// Spawns the next queued proof task on the executor with the given input, if there are any
    /// transactions available.
    ///
    /// This will return an error if a transaction must be created on-demand and the consistent view
    /// provider fails.
    pub fn try_spawn_next(&mut self) -> ProviderResult<()> {
        let Some(task) = self.pending_on_demand.pop_front() else { return Ok(()) };

        let Some(proof_task_tx) = self.get_or_create_tx()? else {
            // if there are no txs available, requeue the proof task
            self.pending_on_demand.push_front(task);
            return Ok(())
        };

        let tx_sender = self.tx_sender.clone();
        self.executor.spawn_blocking(move || match task {
            ProofTaskKind::StorageProof(input, sender) => {
                proof_task_tx.storage_proof(input, sender, tx_sender);
            }
            ProofTaskKind::BlindedAccountNode(path, sender) => {
                proof_task_tx.blinded_account_node(path, sender, tx_sender);
            }
            ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                proof_task_tx.blinded_storage_node(account, path, sender, tx_sender);
            }
        });

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    ///
    /// # Task Routing
    ///
    /// - **Storage Proofs**: Routed to pre-spawned worker pool via bounded channel
    ///   - If channel is full, falls back to on-demand spawn
    /// - **Blinded Nodes**: Queued for on-demand execution (original behavior)
    ///
    /// # Worker Pool Lifecycle
    ///
    /// On termination, `storage_work_tx` is dropped, closing the channel and signaling
    /// all workers to shut down gracefully.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => {
                    match message {
                        ProofTaskMessage::QueueTask(task) => {
                            match task {
                                // ==================== STORAGE PROOF ROUTING ====================
                                ProofTaskKind::StorageProof(input, sender) => {
                                    #[cfg(feature = "metrics")]
                                    {
                                        self.metrics.storage_proofs += 1;
                                    }

                                    // Try to send to worker pool first
                                    match self
                                        .storage_work_tx
                                        .try_send(StorageProofJob { input, result_sender: sender })
                                    {
                                        Ok(_) => {
                                            // Successfully queued to worker pool
                                            tracing::trace!(
                                                target: "trie::proof_task",
                                                "Storage proof dispatched to worker pool"
                                            );
                                        }
                                        Err(crossbeam_channel::TrySendError::Full(job)) => {
                                            // Channel full - fall back to on-demand spawn
                                            tracing::debug!(
                                                target: "trie::proof_task",
                                                "Worker pool queue full, spawning on-demand"
                                            );

                                            #[cfg(feature = "metrics")]
                                            {
                                                self.metrics.on_demand_fallback += 1;
                                            }

                                            // Queue for on-demand execution
                                            self.pending_on_demand.push_back(
                                                ProofTaskKind::StorageProof(
                                                    job.input,
                                                    job.result_sender,
                                                ),
                                            );
                                        }
                                        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                            // Workers shut down - this should not happen
                                            tracing::error!(
                                                target: "trie::proof_task",
                                                "Worker pool disconnected unexpectedly"
                                            );
                                            return Err(reth_storage_errors::provider::ProviderError::Database(
                                            reth_db_api::DatabaseError::Other("Worker pool disconnected".into())
                                        ))
                                        }
                                    }
                                }

                                // ==================== BLINDED NODE ROUTING ====================
                                ProofTaskKind::BlindedAccountNode(_, _) => {
                                    #[cfg(feature = "metrics")]
                                    {
                                        self.metrics.account_nodes += 1;
                                    }
                                    self.queue_proof_task(task);
                                }
                                ProofTaskKind::BlindedStorageNode(_, _, _) => {
                                    #[cfg(feature = "metrics")]
                                    {
                                        self.metrics.storage_nodes += 1;
                                    }
                                    self.queue_proof_task(task);
                                }
                            }
                        }
                        ProofTaskMessage::Transaction(tx) => {
                            // Return transaction to on-demand pool
                            self.on_demand_txs.push(tx);
                        }
                        ProofTaskMessage::Terminate => {
                            // Drop storage_work_tx to signal workers to shut down
                            drop(self.storage_work_tx);

                            tracing::info!(
                                target: "trie::proof_task",
                                storage_worker_count = self.storage_worker_count,
                                "Shutting down proof task manager, signaling workers to terminate"
                            );

                            // Record metrics before terminating
                            #[cfg(feature = "metrics")]
                            self.metrics.record();

                            return Ok(())
                        }
                    }
                }
                // All senders are disconnected, so we can terminate
                // However this should never happen, as this struct stores a sender
                Err(_) => return Ok(()),
            };

            // Try spawning on-demand tasks only (storage proofs handled by worker pool)
            self.try_spawn_next()?;
        }
    }
}

/// Type alias for the factory tuple returned by `create_factories`
type ProofFactories<'a, Tx> = (
    InMemoryTrieCursorFactory<DatabaseTrieCursorFactory<&'a Tx>, &'a TrieUpdatesSorted>,
    HashedPostStateCursorFactory<DatabaseHashedCursorFactory<&'a Tx>, &'a HashedPostStateSorted>,
);

/// This contains all information shared between all storage proof instances.
#[derive(Debug)]
pub struct ProofTaskTx<Tx> {
    /// The tx that is reused for proof calculations.
    tx: Tx,

    /// Trie updates, prefix sets, and state updates
    task_ctx: ProofTaskCtx,

    /// Identifier for the tx within the context of a single [`ProofTaskManager`], used only for
    /// tracing.
    id: usize,
}

impl<Tx> ProofTaskTx<Tx> {
    /// Initializes a [`ProofTaskTx`] using the given transaction and a [`ProofTaskCtx`]. The id is
    /// used only for tracing.
    const fn new(tx: Tx, task_ctx: ProofTaskCtx, id: usize) -> Self {
        Self { tx, task_ctx, id }
    }
}

impl<Tx> ProofTaskTx<Tx>
where
    Tx: DbTx,
{
    fn create_factories(&self) -> ProofFactories<'_, Tx> {
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(&self.tx),
            self.task_ctx.nodes_sorted.as_ref(),
        );

        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(&self.tx),
            self.task_ctx.state_sorted.as_ref(),
        );

        (trie_cursor_factory, hashed_cursor_factory)
    }

    /// Compute storage proof without consuming self (for worker pool reuse).
    ///
    /// # Purpose
    ///
    /// This method enables transaction reuse in the storage worker pool. Unlike the
    /// original `storage_proof(self, ...)` which consumes self and returns the
    /// transaction to a pool, this method:
    ///
    /// 1. Borrows self immutably
    /// 2. Computes the proof using the owned transaction
    /// 3. Returns only the result (transaction remains owned)
    /// 4. Can be called repeatedly on the same [`ProofTaskTx`] instance
    ///
    /// # Usage
    ///
    /// This is called exclusively by storage workers in the worker pool. On-demand
    /// execution still uses the original `storage_proof(self, ...)` method which
    /// consumes self and returns the transaction.
    ///
    /// # Performance
    ///
    /// By reusing the same transaction and cursor factories across multiple proofs:
    /// - Eliminates per-proof transaction creation overhead
    /// - Avoids message passing to return transactions
    /// - Reduces memory allocations for cursor factories
    fn compute_storage_proof(&self, input: &StorageProofInput) -> StorageProofResult {
        // ==================== SETUP ====================

        // Create cursor factories (same as original implementation)
        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        // Get or create added/removed keys context
        let multi_added_removed_keys = input
            .multi_added_removed_keys
            .clone()
            .unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&input.hashed_address);

        let span = tracing::trace_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            hashed_address = ?input.hashed_address,
            // Worker ID embedded in ProofTaskTx for trace correlation
            worker_id = self.id,
        );
        let _guard = span.enter();

        let target_slots_len = input.target_slots.len();
        let proof_start = Instant::now();

        // Compute raw storage multiproof (identical to original)
        let raw_proof_result = StorageProof::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            input.hashed_address,
        )
        .with_prefix_set_mut(PrefixSetMut::from(input.prefix_set.iter().copied()))
        .with_branch_node_masks(input.with_branch_node_masks)
        .with_added_removed_keys(added_removed_keys)
        .storage_multiproof(input.target_slots.clone())
        .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        // Decode proof into DecodedStorageMultiProof
        let decoded_result = raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    input.hashed_address, e
                ))
            })
        });

        trace!(
            target: "trie::proof_task",
            hashed_address = ?input.hashed_address,
            prefix_set_len = input.prefix_set.len(),
            target_slots = target_slots_len,
            proof_time_us = proof_start.elapsed().as_micros(),
            worker_id = self.id,
            "Completed storage proof calculation"
        );

        decoded_result

        // NOTE: self is NOT consumed - transaction remains owned by worker
        // No ProofTaskMessage::Transaction sent
    }

    /// Calculates a storage proof for the given hashed address, and desired prefix set.
    ///
    /// **ON-DEMAND VARIANT** - Consumes self, returns transaction to pool.
    ///
    /// This method is NO LONGER CALLED for storage proofs from the worker pool,
    /// but is kept for:
    /// 1. Backward compatibility with any direct callers
    /// 2. Future use cases that need one-off storage proofs
    /// 3. Tests that rely on the transaction return mechanism
    fn storage_proof(
        self,
        input: StorageProofInput,
        result_sender: Sender<StorageProofResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        trace!(
            target: "trie::proof_task",
            hashed_address=?input.hashed_address,
            "Starting storage proof task calculation"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();
        let multi_added_removed_keys = input
            .multi_added_removed_keys
            .unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&input.hashed_address);

        let span = tracing::trace_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            hashed_address=?input.hashed_address,
            // Add a unique id because we often have parallel storage proof calculations for the
            // same hashed address, and we want to differentiate them during trace analysis.
            span_id=self.id,
        );
        let span_guard = span.enter();

        let target_slots_len = input.target_slots.len();
        let proof_start = Instant::now();

        let raw_proof_result = StorageProof::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            input.hashed_address,
        )
        .with_prefix_set_mut(PrefixSetMut::from(input.prefix_set.iter().copied()))
        .with_branch_node_masks(input.with_branch_node_masks)
        .with_added_removed_keys(added_removed_keys)
        .storage_multiproof(input.target_slots)
        .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        drop(span_guard);

        let decoded_result = raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    input.hashed_address, e
                ))
            })
        });

        trace!(
            target: "trie::proof_task",
            hashed_address=?input.hashed_address,
            prefix_set = ?input.prefix_set.len(),
            target_slots = ?target_slots_len,
            proof_time = ?proof_start.elapsed(),
            "Completed storage proof task calculation"
        );

        // send the result back
        if let Err(error) = result_sender.send(decoded_result) {
            debug!(
                target: "trie::proof_task",
                hashed_address = ?input.hashed_address,
                ?error,
                task_time = ?proof_start.elapsed(),
                "Storage proof receiver is dropped, discarding the result"
            );
        }

        // send the tx back
        let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
    }

    /// Retrieves blinded account node by path.
    fn blinded_account_node(
        self,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        trace!(
            target: "trie::proof_task",
            ?path,
            "Starting blinded account node retrieval"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
            trie_cursor_factory,
            hashed_cursor_factory,
            self.task_ctx.prefix_sets.clone(),
        );

        let start = Instant::now();
        let result = blinded_provider_factory.account_node_provider().trie_node(&path);
        trace!(
            target: "trie::proof_task",
            ?path,
            elapsed = ?start.elapsed(),
            "Completed blinded account node retrieval"
        );

        if let Err(error) = result_sender.send(result) {
            tracing::error!(
                target: "trie::proof_task",
                ?path,
                ?error,
                "Failed to send blinded account node result"
            );
        }

        // send the tx back
        let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
    }

    /// Retrieves blinded storage node of the given account by path.
    fn blinded_storage_node(
        self,
        account: B256,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        trace!(
            target: "trie::proof_task",
            ?account,
            ?path,
            "Starting blinded storage node retrieval"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
            trie_cursor_factory,
            hashed_cursor_factory,
            self.task_ctx.prefix_sets.clone(),
        );

        let start = Instant::now();
        let result = blinded_provider_factory.storage_node_provider(account).trie_node(&path);
        trace!(
            target: "trie::proof_task",
            ?account,
            ?path,
            elapsed = ?start.elapsed(),
            "Completed blinded storage node retrieval"
        );

        if let Err(error) = result_sender.send(result) {
            tracing::error!(
                target: "trie::proof_task",
                ?account,
                ?path,
                ?error,
                "Failed to send blinded storage node result"
            );
        }

        // send the tx back
        let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
    }
}

/// This represents an input for a storage proof.
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

/// Data used for initializing cursor factories that is shared across all storage proof instances.
#[derive(Debug, Clone)]
pub struct ProofTaskCtx {
    /// The sorted collection of cached in-memory intermediate trie nodes that can be reused for
    /// computation.
    nodes_sorted: Arc<TrieUpdatesSorted>,
    /// The sorted in-memory overlay hashed state.
    state_sorted: Arc<HashedPostStateSorted>,
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `state_sorted` might be present here,
    /// if we have cached nodes for them.
    prefix_sets: Arc<TriePrefixSetsMut>,
}

impl ProofTaskCtx {
    /// Creates a new [`ProofTaskCtx`] with the given sorted nodes and state.
    pub const fn new(
        nodes_sorted: Arc<TrieUpdatesSorted>,
        state_sorted: Arc<HashedPostStateSorted>,
        prefix_sets: Arc<TriePrefixSetsMut>,
    ) -> Self {
        Self { nodes_sorted, state_sorted, prefix_sets }
    }
}

/// Message used to communicate with [`ProofTaskManager`].
#[derive(Debug)]
pub enum ProofTaskMessage<Tx> {
    /// A request to queue a proof task.
    QueueTask(ProofTaskKind),
    /// A returned database transaction.
    Transaction(ProofTaskTx<Tx>),
    /// A request to terminate the proof task manager.
    Terminate,
}

/// Proof task kind.
///
/// When queueing a task using [`ProofTaskMessage::QueueTask`], this enum
/// specifies the type of proof task to be executed.
#[derive(Debug)]
pub enum ProofTaskKind {
    /// A storage proof request.
    StorageProof(StorageProofInput, Sender<StorageProofResult>),
    /// A blinded account node request.
    BlindedAccountNode(Nibbles, Sender<TrieNodeProviderResult>),
    /// A blinded storage node request.
    BlindedStorageNode(B256, Nibbles, Sender<TrieNodeProviderResult>),
}

/// A handle that wraps a single proof task sender that sends a terminate message on `Drop` if the
/// number of active handles went to zero.
#[derive(Debug)]
pub struct ProofTaskManagerHandle<Tx> {
    /// The sender for the proof task manager.
    sender: Sender<ProofTaskMessage<Tx>>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
}

impl<Tx> ProofTaskManagerHandle<Tx> {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub fn new(sender: Sender<ProofTaskMessage<Tx>>, active_handles: Arc<AtomicUsize>) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self { sender, active_handles }
    }

    /// Queues a task to the proof task manager.
    pub fn queue_task(&self, task: ProofTaskKind) -> Result<(), SendError<ProofTaskMessage<Tx>>> {
        self.sender.send(ProofTaskMessage::QueueTask(task))
    }

    /// Terminates the proof task manager.
    pub fn terminate(&self) {
        let _ = self.sender.send(ProofTaskMessage::Terminate);
    }
}

impl<Tx> Clone for ProofTaskManagerHandle<Tx> {
    fn clone(&self) -> Self {
        Self::new(self.sender.clone(), self.active_handles.clone())
    }
}

impl<Tx> Drop for ProofTaskManagerHandle<Tx> {
    fn drop(&mut self) {
        // Decrement the number of active handles and terminate the manager if it was the last
        // handle.
        if self.active_handles.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.terminate();
        }
    }
}

impl<Tx: DbTx> TrieNodeProviderFactory for ProofTaskManagerHandle<Tx> {
    type AccountNodeProvider = ProofTaskTrieNodeProvider<Tx>;
    type StorageNodeProvider = ProofTaskTrieNodeProvider<Tx>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { sender: self.sender.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, sender: self.sender.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider<Tx> {
    /// Blinded account trie node provider.
    AccountNode {
        /// Sender to the proof task.
        sender: Sender<ProofTaskMessage<Tx>>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Sender to the proof task.
        sender: Sender<ProofTaskMessage<Tx>>,
    },
}

impl<Tx: DbTx> TrieNodeProvider for ProofTaskTrieNodeProvider<Tx> {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = channel();
        match self {
            Self::AccountNode { sender } => {
                let _ = sender.send(ProofTaskMessage::QueueTask(
                    ProofTaskKind::BlindedAccountNode(*path, tx),
                ));
            }
            Self::StorageNode { sender, account } => {
                let _ = sender.send(ProofTaskMessage::QueueTask(
                    ProofTaskKind::BlindedStorageNode(*account, *path, tx),
                ));
            }
        }

        rx.recv().unwrap()
    }
}
