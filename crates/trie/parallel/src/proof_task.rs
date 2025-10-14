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
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    prefix_set::TriePrefixSetsMut,
    proof::{ProofTrieNodeProviderFactory, StorageProof},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
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
use tracing::trace;

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Internal message for storage workers.
///
/// This is NOT exposed publicly. External callers use `ProofTaskKind::StorageProof` or
/// `ProofTaskKind::BlindedStorageNode` which are routed through the manager's `std::mpsc` channel.
#[derive(Debug)]
enum StorageWorkerJob {
    /// Storage proof computation request
    StorageProof {
        /// Storage proof input parameters
        input: StorageProofInput,
        /// Channel to send result back to original caller
        result_sender: Sender<StorageProofResult>,
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

impl StorageWorkerJob {
    /// Sends an error back to the caller when worker pool is unavailable.
    ///
    /// Returns `Ok(())` if the error was sent successfully, or `Err(())` if the receiver was
    /// dropped.
    fn send_worker_unavailable_error(&self) -> Result<(), ()> {
        let error =
            ParallelStateRootError::Other("Storage proof worker pool unavailable".to_string());

        match self {
            Self::StorageProof { result_sender, .. } => {
                result_sender.send(Err(error)).map_err(|_| ())
            }
            Self::BlindedStorageNode { result_sender, .. } => result_sender
                .send(Err(SparseTrieError::from(SparseTrieErrorKind::Other(Box::new(error)))))
                .map_err(|_| ()),
        }
    }
}

/// Manager for coordinating proof request execution across different task types.
///
/// # Architecture
///
/// This manager handles two distinct execution paths:
///
/// 1. **Storage Worker Pool** (for storage trie operations):
///    - Pre-spawned workers with dedicated long-lived transactions
///    - Handles `StorageProof` and `BlindedStorageNode` requests
///    - Tasks queued via crossbeam unbounded channel
///    - Workers continuously process without transaction overhead
///    - Unbounded queue ensures all storage proofs benefit from transaction reuse
///
/// 2. **On-Demand Execution** (for account trie operations):
///    - Lazy transaction creation for `BlindedAccountNode` requests
///    - Transactions returned to pool after use for reuse
///
/// # Public Interface
///
/// The public interface through `ProofTaskManagerHandle` allows external callers to:
/// - Submit tasks via `queue_task(ProofTaskKind)`
/// - Use standard `std::mpsc` message passing
/// - Receive consistent return types and error handling
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    /// Sender for storage worker jobs to worker pool.
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,

    /// Number of storage workers successfully spawned.
    ///
    /// May be less than requested if concurrency limits reduce the worker budget.
    storage_worker_count: usize,

    /// Max number of database transactions to create for on-demand account trie operations.
    max_concurrency: usize,

    /// Number of database transactions created for on-demand operations.
    total_transactions: usize,

    /// Proof tasks pending execution (account trie operations only).
    pending_tasks: VecDeque<ProofTaskKind>,

    /// The proof task transactions, containing owned cursor factories that are reused for proof
    /// calculation (account trie operations only).
    proof_task_txs: Vec<ProofTaskTx<FactoryTx<Factory>>>,

    /// Consistent view provider used for creating transactions on-demand.
    view: ConsistentDbView<Factory>,

    /// Proof task context shared across all proof tasks.
    task_ctx: ProofTaskCtx,

    /// The underlying handle from which to spawn proof tasks.
    executor: Handle,

    /// Receives proof task requests from [`ProofTaskManagerHandle`].
    proof_task_rx: Receiver<ProofTaskMessage<FactoryTx<Factory>>>,

    /// Internal channel for on-demand tasks to return transactions after use.
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

/// Worker loop for storage trie operations.
///
/// # Lifecycle
///
/// Each worker:
/// 1. Receives `StorageWorkerJob` from crossbeam unbounded channel
/// 2. Computes result using its dedicated long-lived transaction
/// 3. Sends result directly to original caller via `std::mpsc`
/// 4. Repeats until channel closes (graceful shutdown)
///
/// # Transaction Reuse
///
/// Reuses the same transaction and cursor factories across multiple operations
/// to avoid transaction creation and cursor factory setup overhead.
///
/// # Panic Safety
///
/// If this function panics, the worker thread terminates but other workers
/// continue operating and the system degrades gracefully.
///
/// # Shutdown
///
/// Worker shuts down when the crossbeam channel closes (all senders dropped).
fn storage_worker_loop<Tx>(
    proof_tx: ProofTaskTx<Tx>,
    work_rx: CrossbeamReceiver<StorageWorkerJob>,
    worker_id: usize,
) where
    Tx: DbTx,
{
    tracing::debug!(
        target: "trie::proof_task",
        worker_id,
        "Storage worker started"
    );

    // Create factories once at worker startup to avoid recreation overhead.
    let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();

    // Create blinded provider factory once for all blinded node requests
    let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
        trie_cursor_factory.clone(),
        hashed_cursor_factory.clone(),
        proof_tx.task_ctx.prefix_sets.clone(),
    );

    let mut storage_proofs_processed = 0u64;
    let mut storage_nodes_processed = 0u64;

    while let Ok(job) = work_rx.recv() {
        match job {
            StorageWorkerJob::StorageProof { input, result_sender } => {
                let hashed_address = input.hashed_address;

                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    hashed_address = ?hashed_address,
                    prefix_set_len = input.prefix_set.len(),
                    target_slots = input.target_slots.len(),
                    "Processing storage proof"
                );

                let proof_start = Instant::now();
                let result = proof_tx.compute_storage_proof(
                    input,
                    trie_cursor_factory.clone(),
                    hashed_cursor_factory.clone(),
                );

                let proof_elapsed = proof_start.elapsed();
                storage_proofs_processed += 1;

                if result_sender.send(result).is_err() {
                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        hashed_address = ?hashed_address,
                        storage_proofs_processed,
                        "Storage proof receiver dropped, discarding result"
                    );
                }

                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    hashed_address = ?hashed_address,
                    proof_time_us = proof_elapsed.as_micros(),
                    total_processed = storage_proofs_processed,
                    "Storage proof completed"
                );
            }

            StorageWorkerJob::BlindedStorageNode { account, path, result_sender } => {
                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    ?account,
                    ?path,
                    "Processing blinded storage node"
                );

                let start = Instant::now();
                let result =
                    blinded_provider_factory.storage_node_provider(account).trie_node(&path);
                let elapsed = start.elapsed();

                storage_nodes_processed += 1;

                if result_sender.send(result).is_err() {
                    tracing::debug!(
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
    }

    tracing::debug!(
        target: "trie::proof_task",
        worker_id,
        storage_proofs_processed,
        storage_nodes_processed,
        "Storage worker shutting down"
    );
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    /// Creates a new [`ProofTaskManager`] with pre-spawned storage proof workers.
    ///
    /// The `storage_worker_count` determines how many storage workers to spawn, and
    /// `max_concurrency` determines the limit for on-demand operations (blinded account nodes).
    /// These are now independent - storage workers are spawned as requested, and on-demand
    /// operations use a separate concurrency pool for blinded account nodes.
    /// Returns an error if the underlying provider fails to create the transactions required for
    /// spawning workers.
    pub fn new(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        max_concurrency: usize,
        storage_worker_count: usize,
    ) -> ProviderResult<Self> {
        let (tx_sender, proof_task_rx) = channel();

        // Use unbounded channel to ensure all storage operations are queued to workers.
        // This maintains transaction reuse benefits and avoids fallback to on-demand execution.
        let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();

        tracing::info!(
            target: "trie::proof_task",
            storage_worker_count,
            max_concurrency,
            "Initializing storage worker pool with unbounded queue"
        );

        let mut spawned_workers = 0;
        for worker_id in 0..storage_worker_count {
            let provider_ro = view.provider_ro()?;

            let tx = provider_ro.into_tx();
            let proof_task_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id);
            let work_rx = storage_work_rx.clone();

            executor.spawn_blocking(move || storage_worker_loop(proof_task_tx, work_rx, worker_id));

            spawned_workers += 1;

            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                spawned_workers,
                "Storage worker spawned successfully"
            );
        }

        Ok(Self {
            storage_work_tx,
            storage_worker_count: spawned_workers,
            max_concurrency,
            total_transactions: 0,
            pending_tasks: VecDeque::new(),
            proof_task_txs: Vec::with_capacity(max_concurrency),
            view,
            task_ctx,
            executor,
            proof_task_rx,
            tx_sender,
            active_handles: Arc::new(AtomicUsize::new(0)),

            #[cfg(feature = "metrics")]
            metrics: ProofTaskMetrics::default(),
        })
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
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
        self.pending_tasks.push_back(task);
    }

    /// Gets either the next available transaction, or creates a new one if all are in use and the
    /// total number of transactions created is less than the max concurrency.
    pub fn get_or_create_tx(&mut self) -> ProviderResult<Option<ProofTaskTx<FactoryTx<Factory>>>> {
        if let Some(proof_task_tx) = self.proof_task_txs.pop() {
            return Ok(Some(proof_task_tx));
        }

        // if we can create a new tx within our concurrency limits, create one on-demand
        if self.total_transactions < self.max_concurrency {
            let provider_ro = self.view.provider_ro()?;
            let tx = provider_ro.into_tx();
            self.total_transactions += 1;
            return Ok(Some(ProofTaskTx::new(tx, self.task_ctx.clone(), self.total_transactions)));
        }

        Ok(None)
    }

    /// Spawns the next queued proof task on the executor with the given input, if there are any
    /// transactions available.
    ///
    /// This will return an error if a transaction must be created on-demand and the consistent view
    /// provider fails.
    pub fn try_spawn_next(&mut self) -> ProviderResult<()> {
        let Some(task) = self.pending_tasks.pop_front() else { return Ok(()) };

        let Some(proof_task_tx) = self.get_or_create_tx()? else {
            // if there are no txs available, requeue the proof task
            self.pending_tasks.push_front(task);
            return Ok(())
        };

        let tx_sender = self.tx_sender.clone();
        self.executor.spawn_blocking(move || match task {
            ProofTaskKind::BlindedAccountNode(path, sender) => {
                proof_task_tx.blinded_account_node(path, sender, tx_sender);
            }
            // Storage trie operations should never reach here as they're routed to worker pool
            ProofTaskKind::BlindedStorageNode(_, _, _) | ProofTaskKind::StorageProof(_, _) => {
                unreachable!("Storage trie operations should be routed to worker pool")
            }
        });

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    ///
    /// # Task Routing
    ///
    /// - **Storage Trie Operations** (`StorageProof` and `BlindedStorageNode`): Routed to
    ///   pre-spawned worker pool via unbounded channel.
    /// - **Account Trie Operations** (`BlindedAccountNode`): Queued for on-demand execution via
    ///   `pending_tasks`.
    ///
    /// # Shutdown
    ///
    /// On termination, `storage_work_tx` is dropped, closing the channel and
    /// signaling all workers to shut down gracefully.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => {
                    match message {
                        ProofTaskMessage::QueueTask(task) => match task {
                            ProofTaskKind::StorageProof(input, sender) => {
                                match self.storage_work_tx.send(StorageWorkerJob::StorageProof {
                                    input,
                                    result_sender: sender,
                                }) {
                                    Ok(_) => {
                                        tracing::trace!(
                                            target: "trie::proof_task",
                                            "Storage proof dispatched to worker pool"
                                        );
                                    }
                                    Err(crossbeam_channel::SendError(job)) => {
                                        tracing::error!(
                                            target: "trie::proof_task",
                                            storage_worker_count = self.storage_worker_count,
                                            "Worker pool disconnected, cannot process storage proof"
                                        );

                                        // Send error back to caller
                                        let _ = job.send_worker_unavailable_error();
                                    }
                                }
                            }

                            ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.storage_nodes += 1;
                                }

                                match self.storage_work_tx.send(
                                    StorageWorkerJob::BlindedStorageNode {
                                        account,
                                        path,
                                        result_sender: sender,
                                    },
                                ) {
                                    Ok(_) => {
                                        tracing::trace!(
                                            target: "trie::proof_task",
                                            ?account,
                                            ?path,
                                            "Blinded storage node dispatched to worker pool"
                                        );
                                    }
                                    Err(crossbeam_channel::SendError(job)) => {
                                        tracing::warn!(
                                            target: "trie::proof_task",
                                            storage_worker_count = self.storage_worker_count,
                                            ?account,
                                            ?path,
                                            "Worker pool disconnected, cannot process blinded storage node"
                                        );

                                        // Send error back to caller
                                        let _ = job.send_worker_unavailable_error();
                                    }
                                }
                            }

                            ProofTaskKind::BlindedAccountNode(_, _) => {
                                // Route account trie operations to pending_tasks
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.account_nodes += 1;
                                }
                                self.queue_proof_task(task);
                            }
                        },
                        ProofTaskMessage::Transaction(tx) => {
                            // Return transaction to pending_tasks pool
                            self.proof_task_txs.push(tx);
                        }
                        ProofTaskMessage::Terminate => {
                            // Drop storage_work_tx to signal workers to shut down
                            drop(self.storage_work_tx);

                            tracing::debug!(
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

            // Try spawning pending account trie tasks
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
    #[inline]
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

    /// Compute storage proof with pre-created factories.
    ///
    /// Accepts cursor factories as parameters to allow reuse across multiple proofs.
    /// Used by storage workers in the worker pool to avoid factory recreation
    /// overhead on each proof computation.
    #[inline]
    fn compute_storage_proof(
        &self,
        input: StorageProofInput,
        trie_cursor_factory: impl TrieCursorFactory,
        hashed_cursor_factory: impl HashedCursorFactory,
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

        let span = tracing::trace_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            hashed_address = ?hashed_address,
            worker_id = self.id,
        );
        let _span_guard = span.enter();

        let proof_start = Instant::now();

        // Compute raw storage multiproof
        let raw_proof_result =
            StorageProof::new_hashed(trie_cursor_factory, hashed_cursor_factory, hashed_address)
                .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().copied()))
                .with_branch_node_masks(with_branch_node_masks)
                .with_added_removed_keys(added_removed_keys)
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::B256Map;
    use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
    use reth_trie_common::{
        prefix_set::TriePrefixSetsMut, updates::TrieUpdatesSorted, HashedAccountsSorted,
        HashedPostStateSorted,
    };
    use std::sync::Arc;
    use tokio::{runtime::Builder, task};

    fn test_ctx() -> ProofTaskCtx {
        ProofTaskCtx::new(
            Arc::new(TrieUpdatesSorted::default()),
            Arc::new(HashedPostStateSorted::new(
                HashedAccountsSorted::default(),
                B256Map::default(),
            )),
            Arc::new(TriePrefixSetsMut::default()),
        )
    }

    /// Ensures `max_concurrency` is independent of storage workers.
    #[test]
    fn proof_task_manager_independent_pools() {
        let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = tokio::runtime::Handle::current();
            let factory = create_test_provider_factory();
            let view = ConsistentDbView::new(factory, None);
            let ctx = test_ctx();

            let manager = ProofTaskManager::new(handle.clone(), view, ctx, 1, 5).unwrap();
            // With storage_worker_count=5, we get exactly 5 workers
            assert_eq!(manager.storage_worker_count, 5);
            // max_concurrency=1 is for on-demand operations only
            assert_eq!(manager.max_concurrency, 1);

            drop(manager);
            task::yield_now().await;
        });
    }
}
