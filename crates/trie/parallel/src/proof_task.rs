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
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use crossbeam_channel::{self, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::{TriePrefixSets, TriePrefixSetsMut},
    proof::{ProofTrieNodeProviderFactory, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    DecodedMultiProof, DecodedStorageMultiProof, HashedPostStateSorted, MultiProofTargets, Nibbles,
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
        mpsc::{channel, Receiver as MpscReceiver, SendError, Sender as MpscSender},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::{debug, trace, warn};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

pub(crate) type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Creates a standardized error for when the storage proof manager is closed.
#[inline]
fn storage_manager_closed_error() -> ParallelStateRootError {
    ParallelStateRootError::Other("storage manager closed".into())
}

/// Executes an account multiproof task in a worker thread.
///
/// This function coordinates with the storage manager to compute storage proofs for all
/// accounts in the multiproof, then assembles the final account multiproof by fetching
/// storage proofs on-demand during account trie traversal.
fn execute_account_multiproof_worker<Tx: DbTx>(
    input: AccountMultiproofInput<Tx>,
    proof_tx: &ProofTaskTx<Tx>,
) {
    let AccountMultiproofInput {
        targets,
        prefix_sets,
        collect_branch_node_masks,
        multi_added_removed_keys,
        storage_proof_handle,
        result_sender,
    } = input;

    // Queue ALL storage proof requests to storage manager
    let mut storage_receivers: B256Map<
        CrossbeamReceiver<Result<DecodedStorageMultiProof, ParallelStateRootError>>,
    > = B256Map::default();
    for (address, slots) in targets.iter() {
        if slots.is_empty() {
            continue;
        }

        let (sender, receiver) = crossbeam_channel::unbounded();
        let prefix_set = prefix_sets.storage_prefix_sets.get(address).cloned().unwrap_or_default();
        let storage_input = StorageProofInput::new(
            *address,
            prefix_set,
            Arc::new(slots.clone()), // Arc clone is cheap (reference count only)
            collect_branch_node_masks,
            multi_added_removed_keys.clone(),
        );

        if storage_proof_handle
            .queue_task(ProofTaskKind::StorageProof(storage_input, sender))
            .is_err()
        {
            debug!(
                target: "trie::proof_task",
                ?address,
                "Storage manager closed, cannot queue proof"
            );
            let _ = result_sender.send(Err(storage_manager_closed_error()));
            return;
        }

        storage_receivers.insert(*address, receiver);
    }

    //  Build account multiproof, fetching storage proofs on-demand during trie traversal
    let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();

    let result = crate::proof::build_account_multiproof_with_storage(
        trie_cursor_factory,
        hashed_cursor_factory,
        targets,
        prefix_sets,
        storage_receivers, // ← Pass receivers directly for on-demand fetching
        collect_branch_node_masks,
        multi_added_removed_keys,
    );

    if result_sender.send(result.map(|(multiproof, _stats)| multiproof)).is_err() {
        warn!(
            target: "trie::proof_task",
            "Account multiproof result discarded - receiver dropped"
        );
    }
}

/// Proof job dispatched to the storage worker pool.
struct ProofJob {
    input: StorageProofInput,
    result_tx: CrossbeamSender<StorageProofResult>,
    #[cfg(feature = "metrics")]
    enqueued_at: Instant,
}

/// Account multiproof job dispatched to the account worker pool.
struct AccountProofJob<Tx> {
    input: AccountMultiproofInput<Tx>,
    #[cfg(feature = "metrics")]
    enqueued_at: Instant,
}

/// A task that manages sending multiproof requests to a number of tasks that have longer-running
/// database transactions
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    /// Channel for dispatching storage proof work to workers
    storage_work_tx: crossbeam_channel::Sender<ProofJob>,
    /// Channel for dispatching account multiproof work to workers
    account_work_tx: crossbeam_channel::Sender<AccountProofJob<FactoryTx<Factory>>>,
    /// Max number of database transactions to create (for blinded nodes)
    max_concurrency: usize,
    /// Number of database transactions created (for blinded nodes)
    total_transactions: usize,
    /// Consistent view provider used for creating transactions on-demand
    view: ConsistentDbView<Factory>,
    /// Proof task context shared across all proof tasks
    task_ctx: ProofTaskCtx,
    /// Proof tasks pending execution (blinded nodes only)
    pending_tasks: VecDeque<ProofTaskKind<FactoryTx<Factory>>>,
    /// The underlying handle from which to spawn proof tasks
    executor: Handle,
    /// The proof task transactions for blinded node requests
    proof_task_txs: Vec<ProofTaskTx<FactoryTx<Factory>>>,
    /// A receiver for new proof tasks.
    proof_task_rx: MpscReceiver<ProofTaskMessage<FactoryTx<Factory>>>,
    /// A sender for sending back transactions.
    tx_sender: MpscSender<ProofTaskMessage<FactoryTx<Factory>>>,
    /// The number of active handles.
    ///
    /// Incremented in [`ProofTaskManagerHandle::new`] and decremented in
    /// [`ProofTaskManagerHandle::drop`].
    active_handles: Arc<AtomicUsize>,
    /// Tracks outstanding storage jobs for metrics purposes.
    #[cfg(feature = "metrics")]
    storage_queue_depth: Arc<AtomicUsize>,
    /// Tracks outstanding account jobs for metrics purposes.
    #[cfg(feature = "metrics")]
    account_queue_depth: Arc<AtomicUsize>,
    /// Metrics tracking blinded node fetches.
    #[cfg(feature = "metrics")]
    metrics: ProofTaskMetrics,
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static,
{
    /// Creates a new [`ProofTaskManager`] with the given worker counts and max concurrency.
    ///
    /// Spawns `storage_worker_count` storage proof workers and `account_worker_count` account
    /// multiproof workers upfront that reuse database transactions for the entire block lifetime
    /// (prewarm.rs pattern).
    ///
    /// Returns an error if the consistent view provider fails to create a read-only transaction.
    pub fn new(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        storage_worker_count: usize,
        account_worker_count: usize,
        max_concurrency: usize,
    ) -> ProviderResult<Self> {
        let (tx_sender, proof_task_rx) = channel();
        let (storage_work_tx, storage_work_rx) = crossbeam_channel::unbounded();
        let (account_work_tx, account_work_rx) = crossbeam_channel::unbounded();

        #[cfg(feature = "metrics")]
        let metrics = ProofTaskMetrics::default();
        #[cfg(feature = "metrics")]
        let storage_queue_depth = Arc::new(AtomicUsize::new(0));
        #[cfg(feature = "metrics")]
        let account_queue_depth = Arc::new(AtomicUsize::new(0));

        // Spawn storage workers upfront (prewarm.rs pattern)
        for worker_id in 0..storage_worker_count {
            let provider = view.provider_ro()?;
            let tx = provider.into_tx();
            let proof_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id);
            let storage_work_rx = storage_work_rx.clone();
            #[cfg(feature = "metrics")]
            let storage_queue_depth_clone = Arc::clone(&storage_queue_depth);
            #[cfg(feature = "metrics")]
            let metrics_clone = metrics.clone();

            executor.spawn_blocking(move || {
                debug!(target: "trie::proof_pool", worker_id, "Storage proof worker started");

                // Worker loop - reuse transaction for entire block
                loop {
                    let job: ProofJob = match storage_work_rx.recv() {
                        Ok(item) => item,
                        Err(_) => break, // Channel closed, shutdown
                    };

                    #[cfg(feature = "metrics")]
                    {
                        let depth = storage_queue_depth_clone
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| x.checked_sub(1))
                            .unwrap_or(0)
                            .saturating_sub(1);
                        metrics_clone.record_storage_queue_depth(depth);
                        metrics_clone.record_storage_wait_time(job.enqueued_at.elapsed());
                    }

                    let result = proof_tx.storage_proof_internal(&job.input);
                    let _ = job.result_tx.send(result);
                }

                debug!(target: "trie::proof_pool", worker_id, "Storage proof worker shutdown");
            });
        }

        // Spawn account workers upfront
        for worker_id in 0..account_worker_count {
            let provider = view.provider_ro()?;
            let tx = provider.into_tx();
            let proof_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id + storage_worker_count);
            let account_work_rx = account_work_rx.clone();
            #[cfg(feature = "metrics")]
            let account_queue_depth_clone = Arc::clone(&account_queue_depth);
            #[cfg(feature = "metrics")]
            let metrics_clone = metrics.clone();

            executor.spawn_blocking(move || {
                debug!(target: "trie::proof_pool", worker_id, "Account proof worker started");

                // Worker loop - reuse transaction for entire block
                loop {
                    let job: AccountProofJob<_> = match account_work_rx.recv() {
                        Ok(item) => item,
                        Err(_) => break, /* Channel closed, shutdown
                                          * TODO: should we handle this error? */
                    };

                    #[cfg(feature = "metrics")]
                    {
                        let depth = account_queue_depth_clone
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| x.checked_sub(1))
                            .unwrap_or(0)
                            .saturating_sub(1);
                        metrics_clone.record_account_queue_depth(depth);
                        metrics_clone.record_account_wait_time(job.enqueued_at.elapsed());
                    }

                    // Execute account multiproof (blocks on storage proofs)
                    execute_account_multiproof_worker(job.input, &proof_tx);
                }

                debug!(target: "trie::proof_pool", worker_id, "Account proof worker shutdown");
            });
        }

        Ok(Self {
            storage_work_tx,
            account_work_tx,
            max_concurrency,
            total_transactions: 0,
            view,
            task_ctx,
            pending_tasks: VecDeque::new(),
            executor,
            proof_task_txs: Vec::new(),
            proof_task_rx,
            tx_sender,
            active_handles: Arc::new(AtomicUsize::new(0)),
            #[cfg(feature = "metrics")]
            storage_queue_depth,
            #[cfg(feature = "metrics")]
            account_queue_depth,
            #[cfg(feature = "metrics")]
            metrics,
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
    pub fn queue_proof_task(&mut self, task: ProofTaskKind<FactoryTx<Factory>>) {
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
            ProofTaskKind::StorageProof(input, sender) => {
                proof_task_tx.storage_proof(input, sender, tx_sender);
            }
            ProofTaskKind::BlindedAccountNode(path, sender) => {
                proof_task_tx.blinded_account_node(path, sender, tx_sender);
            }
            ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                proof_task_tx.blinded_storage_node(account, path, sender, tx_sender);
            }
            ProofTaskKind::AccountMultiproof(_) => {
                // AccountMultiproof should never go through spawn-per-task path
                // It should be dispatched to account worker pool in run() loop
                debug!(target: "trie::proof_task", "AccountMultiproof in spawn path - this should not happen");
                // Return transaction to pool
                let _ = tx_sender.send(ProofTaskMessage::Transaction(proof_task_tx));
            }
        });

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => match message {
                    ProofTaskMessage::QueueTask(task) => {
                        match task {
                            ProofTaskKind::StorageProof(input, result_sender) => {
                                // Dispatch to worker pool (non-blocking)
                                let job = ProofJob {
                                    input,
                                    result_tx: result_sender,
                                    #[cfg(feature = "metrics")]
                                    enqueued_at: Instant::now(),
                                };

                                if self.storage_work_tx.send(job).is_err() {
                                    trace!(target: "trie::proof_task", "Storage proof worker pool shut down");
                                } else {
                                    #[cfg(feature = "metrics")]
                                    {
                                        let depth = self
                                            .storage_queue_depth
                                            .fetch_add(1, Ordering::SeqCst) +
                                            1;
                                        self.metrics.record_storage_queue_depth(depth);
                                    }
                                }
                            }
                            ProofTaskKind::BlindedAccountNode(_, _) => {
                                // Track metrics for blinded node requests
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.account_nodes += 1;
                                }
                                // Queue blinded node task for spawn-per-task
                                self.queue_proof_task(task);
                            }
                            ProofTaskKind::BlindedStorageNode(_, _, _) => {
                                // Track metrics for blinded node requests
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.storage_nodes += 1;
                                }
                                // Queue blinded node task for spawn-per-task
                                self.queue_proof_task(task);
                            }
                            ProofTaskKind::AccountMultiproof(input) => {
                                // Dispatch to account worker pool
                                let job = AccountProofJob {
                                    input: *input,
                                    #[cfg(feature = "metrics")]
                                    enqueued_at: Instant::now(),
                                };

                                if self.account_work_tx.send(job).is_err() {
                                    trace!(target: "trie::proof_task", "Account worker pool shut down");
                                } else {
                                    #[cfg(feature = "metrics")]
                                    {
                                        let depth = self
                                            .account_queue_depth
                                            .fetch_add(1, Ordering::SeqCst) +
                                            1;
                                        self.metrics.record_account_queue_depth(depth);
                                    }
                                }
                            }
                        }
                    }
                    ProofTaskMessage::Transaction(tx) => {
                        // return the transaction to the pool (for blinded nodes)
                        self.proof_task_txs.push(tx);
                    }
                    ProofTaskMessage::Terminate => {
                        // Record metrics before terminating
                        #[cfg(feature = "metrics")]
                        self.metrics.record();

                        // Shutdown: drop work channels, workers will exit when channels close
                        drop(self.storage_work_tx);
                        drop(self.account_work_tx);
                        #[cfg(feature = "metrics")]
                        {
                            self.metrics.record_storage_queue_depth(0);
                            self.metrics.record_account_queue_depth(0);
                        }
                        return Ok(());
                    }
                },
                // All senders are disconnected, so we can terminate
                // However this should never happen, as this struct stores a sender
                Err(_) => {
                    // Shutdown workers
                    drop(self.storage_work_tx);
                    drop(self.account_work_tx);
                    #[cfg(feature = "metrics")]
                    {
                        self.metrics.record_storage_queue_depth(0);
                        self.metrics.record_account_queue_depth(0);
                    }
                    return Ok(());
                }
            };

            // try spawning the next blinded node task
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

    /// Generates a storage multiproof using this worker's long-lived transaction.
    ///
    /// This helper is used by the storage worker pool so each worker can service many requests
    /// without recreating database transactions or cursor factories.
    pub fn storage_proof_internal(
        &self,
        input: &StorageProofInput,
    ) -> Result<DecodedStorageMultiProof, ParallelStateRootError> {
        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let multi_added_removed_keys = input
            .multi_added_removed_keys
            .clone()
            .unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&input.hashed_address);

        let raw_proof_result = StorageProof::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            input.hashed_address,
        )
        .with_prefix_set_mut(PrefixSetMut::from(input.prefix_set.iter().copied()))
        .with_branch_node_masks(input.with_branch_node_masks)
        .with_added_removed_keys(added_removed_keys)
        .storage_multiproof((*input.target_slots).clone())
        .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    input.hashed_address, e
                ))
            })
        })
    }

    /// Calculates a storage proof for the given hashed address, and desired prefix set.
    fn storage_proof(
        self,
        input: StorageProofInput,
        result_sender: CrossbeamSender<StorageProofResult>,
        tx_sender: MpscSender<ProofTaskMessage<Tx>>,
    ) {
        trace!(
            target: "trie::proof_task",
            hashed_address=?input.hashed_address,
            "Starting storage proof task calculation"
        );

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

        let decoded_result = self.storage_proof_internal(&input);

        drop(span_guard);

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
        result_sender: CrossbeamSender<TrieNodeProviderResult>,
        tx_sender: MpscSender<ProofTaskMessage<Tx>>,
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
        result_sender: CrossbeamSender<TrieNodeProviderResult>,
        tx_sender: MpscSender<ProofTaskMessage<Tx>>,
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
    pub hashed_address: B256,
    /// The prefix set for the proof calculation.
    pub prefix_set: PrefixSet,
    /// The target slots for the proof calculation.
    /// Arc allows cheap sharing across workers without cloning the entire set.
    pub target_slots: Arc<B256Set>,
    /// Whether or not to collect branch node masks
    pub with_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given hashed address, prefix set, and target
    /// slots.
    pub const fn new(
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: Arc<B256Set>,
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

/// This represents an input for an account multiproof.
#[derive(Debug)]
pub struct AccountMultiproofInput<Tx> {
    /// The proof targets (accounts and their storage slots)
    pub targets: MultiProofTargets,
    /// The frozen prefix sets for account and storage tries
    pub prefix_sets: TriePrefixSets,
    /// Whether to collect branch node masks
    pub collect_branch_node_masks: bool,
    /// Context for retaining extra proofs for added/removed keys
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Handle to the storage proof task manager for coordinating storage proofs
    pub storage_proof_handle: ProofTaskManagerHandle<Tx>,
    /// Sender for returning the multiproof result
    pub result_sender: MpscSender<Result<DecodedMultiProof, ParallelStateRootError>>,
}

impl<Tx> AccountMultiproofInput<Tx> {
    /// Creates a new [`AccountMultiproofInput`] with the given parameters.
    pub const fn new(
        targets: MultiProofTargets,
        prefix_sets: TriePrefixSets,
        collect_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
        storage_proof_handle: ProofTaskManagerHandle<Tx>,
        result_sender: MpscSender<Result<DecodedMultiProof, ParallelStateRootError>>,
    ) -> Self {
        Self {
            targets,
            prefix_sets,
            collect_branch_node_masks,
            multi_added_removed_keys,
            storage_proof_handle,
            result_sender,
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
    QueueTask(ProofTaskKind<Tx>),
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
pub enum ProofTaskKind<Tx = ()> {
    /// A storage proof request.
    StorageProof(StorageProofInput, CrossbeamSender<StorageProofResult>),
    /// A blinded account node request.
    BlindedAccountNode(Nibbles, CrossbeamSender<TrieNodeProviderResult>),
    /// A blinded storage node request.
    BlindedStorageNode(B256, Nibbles, CrossbeamSender<TrieNodeProviderResult>),
    /// An account multiproof request (Phase 1b - not yet activated)
    AccountMultiproof(Box<AccountMultiproofInput<Tx>>),
}

/// A handle that wraps a single proof task sender that sends a terminate message on `Drop` if the
/// number of active handles went to zero.
#[derive(Debug)]
pub struct ProofTaskManagerHandle<Tx> {
    /// The sender for the proof task manager.
    sender: MpscSender<ProofTaskMessage<Tx>>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
}

impl<Tx> ProofTaskManagerHandle<Tx> {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub fn new(sender: MpscSender<ProofTaskMessage<Tx>>, active_handles: Arc<AtomicUsize>) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self { sender, active_handles }
    }

    /// Queues a task to the proof task manager.
    pub fn queue_task(
        &self,
        task: ProofTaskKind<Tx>,
    ) -> Result<(), SendError<ProofTaskMessage<Tx>>> {
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
        sender: MpscSender<ProofTaskMessage<Tx>>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Sender to the proof task.
        sender: MpscSender<ProofTaskMessage<Tx>>,
    },
}

impl<Tx: DbTx> TrieNodeProvider for ProofTaskTrieNodeProvider<Tx> {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = crossbeam_channel::unbounded();
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
    use alloy_primitives::{map::B256Set, B256};
    use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
    use reth_storage_errors::provider::{ConsistentViewError, ProviderError};
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    #[derive(Clone)]
    struct CountingFactory<F> {
        inner: F,
        calls: Arc<AtomicUsize>,
    }

    impl<F> CountingFactory<F> {
        fn new(inner: F, calls: Arc<AtomicUsize>) -> Self {
            Self { inner, calls }
        }
    }

    impl<F> DatabaseProviderFactory for CountingFactory<F>
    where
        F: DatabaseProviderFactory,
    {
        type DB = F::DB;
        type Provider = F::Provider;
        type ProviderRW = F::ProviderRW;

        fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.database_provider_ro()
        }

        fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
            self.inner.database_provider_rw()
        }
    }

    fn default_task_ctx() -> ProofTaskCtx {
        ProofTaskCtx::new(
            Arc::new(TrieUpdatesSorted::default()),
            Arc::new(HashedPostStateSorted::default()),
            Arc::new(TriePrefixSetsMut::default()),
        )
    }

    #[test]
    fn proof_task_manager_new_propagates_consistent_view_error() {
        let factory = create_test_provider_factory();
        let view = ConsistentDbView::new(factory, Some((B256::repeat_byte(0x11), 0)));

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();

        let err = ProofTaskManager::new(rt.handle().clone(), view, task_ctx, 1, 0, 1).unwrap_err();

        assert!(matches!(
            err,
            ProviderError::ConsistentView(ref e) if matches!(**e, ConsistentViewError::Reorged { .. })
        ));
    }

    #[test]
    fn proof_task_manager_spawns_requested_workers_and_processes_tasks() {
        let inner_factory = create_test_provider_factory();
        let calls = Arc::new(AtomicUsize::new(0));
        let counting_factory = CountingFactory::new(inner_factory, Arc::clone(&calls));
        let view = ConsistentDbView::new(counting_factory, None);

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();
        let num_workers = 2usize;
        let manager =
            ProofTaskManager::new(rt.handle().clone(), view, task_ctx, num_workers, 0, 4).unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), num_workers);

        let handle = manager.handle();
        let join_handle = rt.spawn_blocking(move || manager.run());

        let prefix_set = PrefixSetMut::default().freeze();
        let mut receivers = Vec::new();
        for _ in 0..8 {
            let input = StorageProofInput::new(
                B256::ZERO,
                prefix_set.clone(),
                Arc::new(B256Set::default()),
                false,
                None,
            );
            let (sender, receiver) = crossbeam_channel::unbounded();
            handle.queue_task(ProofTaskKind::StorageProof(input, sender)).unwrap();
            receivers.push(receiver);
        }

        for receiver in receivers {
            // We only assert that a result (Ok or Err) arrives for each queued task.
            let _ = receiver.recv().unwrap();
        }

        drop(handle);
        rt.block_on(join_handle).unwrap().unwrap();
    }
}
