//! Proof task management using a pool of pre-warmed database transactions.
//!
//! This module provides proof computation using Tokio's blocking threadpool with
//! transaction reuse via a crossbeam channel pool.

use crate::root::ParallelStateRootError;
use alloy_primitives::{map::B256Set, B256};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError, TrySendError};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderResult,
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
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::{runtime::Handle, sync::Notify};
use tracing::{error, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Creates a new proof task handle with a pre-initialized transaction pool.
///
/// This function creates a pool of database transactions that will be reused across
/// multiple proof tasks. Tasks are queued asynchronously and coordinated via notification,
/// with actual computation dispatched to Tokio's blocking threadpool using the pooled transactions.
pub fn new_proof_task_handle<Factory>(
    executor: Handle,
    view: ConsistentDbView<Factory>,
    task_ctx: ProofTaskCtx,
    max_concurrency: usize,
) -> ProviderResult<ProofTaskManagerHandle<<Factory::Provider as DBProvider>::Tx>>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    let max_concurrency = max_concurrency.max(1);
    let (tx_pool_sender, tx_pool_receiver) = bounded(max_concurrency);
    let pool_notify = Arc::new(Notify::new());

    // Pre-create all transactions upfront
    for worker_id in 0..max_concurrency {
        let provider_ro = view.provider_ro()?;
        let tx = provider_ro.into_tx();
        let proof_task_tx = Arc::new(ProofTaskTx::new(tx, task_ctx.clone(), worker_id));
        tx_pool_sender.send(proof_task_tx).expect("pool channel should have capacity");
    }

    Ok(ProofTaskManagerHandle::new(
        tx_pool_sender,
        tx_pool_receiver,
        pool_notify,
        executor,
        Arc::new(AtomicUsize::new(0)),
        #[cfg(feature = "metrics")]
        Arc::new(ProofTaskMetrics::default()),
    ))
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
    /// Identifier for the tx within the pool, used only for tracing.
    id: usize,
}

impl<Tx> ProofTaskTx<Tx> {
    /// Initializes a [`ProofTaskTx`] using the given transaction and a [`ProofTaskCtx`].
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

    /// Calculates a storage proof for the given hashed address, and desired prefix set.
    fn storage_proof(&self, input: StorageProofInput, result_sender: &Sender<StorageProofResult>) {
        let StorageProofInput {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        } = input;

        trace!(
            target: "trie::proof_task",
            worker_id = self.id,
            hashed_address = ?hashed_address,
            "Starting storage proof task calculation"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();
        let multi_added_removed_keys =
            multi_added_removed_keys.unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&hashed_address);

        let span = tracing::trace_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            hashed_address = ?hashed_address,
            // Add a unique id because we often have parallel storage proof calculations for the
            // same hashed address, and we want to differentiate them during trace analysis.
            span_id = self.id,
        );
        let _span_guard: tracing::span::Entered<'_> = span.enter();

        let target_slots_len = target_slots.len();
        let proof_start = Instant::now();

        let raw_proof_result =
            StorageProof::new_hashed(trie_cursor_factory, hashed_cursor_factory, hashed_address)
                .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().copied()))
                .with_branch_node_masks(with_branch_node_masks)
                .with_added_removed_keys(added_removed_keys)
                .storage_multiproof(target_slots)
                .map_err(|e| ParallelStateRootError::Other(e.to_string()));

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
            worker_id = self.id,
            hashed_address = ?hashed_address,
            prefix_set_len = prefix_set.len(),
            target_slots = target_slots_len,
            proof_time = ?proof_start.elapsed(),
            "Completed storage proof task calculation"
        );

        // Send the result back (log error if receiver dropped)
        if let Err(e) = result_sender.send(decoded_result) {
            error!(
                target: "trie::proof_task",
                worker_id = self.id,
                "Failed to send storage proof result: {:?}",
                e
            );
        }
    }

    /// Retrieves blinded account node by path.
    fn blinded_account_node(&self, path: &Nibbles, result_sender: &Sender<TrieNodeProviderResult>) {
        trace!(
            target: "trie::proof_task",
            worker_id = self.id,
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
        let result = blinded_provider_factory.account_node_provider().trie_node(path);
        trace!(
            target: "trie::proof_task",
            worker_id = self.id,
            ?path,
            elapsed = ?start.elapsed(),
            "Completed blinded account node retrieval"
        );

        if let Err(e) = result_sender.send(result) {
            error!(
                target: "trie::proof_task",
                worker_id = self.id,
                "Failed to send account node result: {:?}",
                e
            );
        }
    }

    /// Retrieves blinded storage node of the given account by path.
    fn blinded_storage_node(
        &self,
        account: &B256,
        path: &Nibbles,
        result_sender: &Sender<TrieNodeProviderResult>,
    ) {
        trace!(
            target: "trie::proof_task",
            worker_id = self.id,
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
        let result = blinded_provider_factory.storage_node_provider(*account).trie_node(path);
        trace!(
            target: "trie::proof_task",
            worker_id = self.id,
            ?account,
            ?path,
            elapsed = ?start.elapsed(),
            "Completed blinded storage node retrieval"
        );

        if let Err(error) = result_sender.send(result) {
            error!(
                target: "trie::proof_task",
                ?account,
                ?path,
                worker_id = self.id,
                ?error,
                "Failed to send storage node result"
            );
        }

        // Note: Transaction return to pool is handled by dispatch_task() after spawn_blocking
        // completes. The Arc<ProofTaskTx> is moved into the closure and returned as the result,
        // then sent back to the pool automatically. No explicit return needed here.
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
    pub target_slots: B256Set,
    /// Whether or not to collect branch node masks
    pub with_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given parameters.
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
    /// The sorted collection of cached in-memory intermediate trie nodes.
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

/// Proof task kind dispatched via [`ProofTaskManagerHandle::queue_task`].
#[derive(Debug)]
pub enum ProofTaskKind {
    /// A storage proof request.
    StorageProof(StorageProofInput, Sender<StorageProofResult>),
    /// A blinded account node request.
    BlindedAccountNode(Nibbles, Sender<TrieNodeProviderResult>),
    /// A blinded storage node request.
    BlindedStorageNode(B256, Nibbles, Sender<TrieNodeProviderResult>),
}

/// A handle for dispatching proof tasks using a transaction pool and Tokio's blocking threadpool.
///
/// Tasks are dispatched directly without an intermediate manager loop.
pub struct ProofTaskManagerHandle<Tx> {
    /// Transaction pool sender (for returning transactions)
    tx_pool_sender: Sender<Arc<ProofTaskTx<Tx>>>,
    /// Transaction pool receiver (for checking out transactions)
    tx_pool_receiver: Receiver<Arc<ProofTaskTx<Tx>>>,
    /// Notifies waiters when a transaction is returned to the pool
    pool_notify: Arc<Notify>,
    /// Tokio executor for spawning blocking tasks
    executor: Handle,
    /// The number of active handles (for metrics).
    active_handles: Arc<AtomicUsize>,
    /// Metrics tracking blinded node fetches.
    #[cfg(feature = "metrics")]
    metrics: Arc<ProofTaskMetrics>,
}

// Manual Debug impl since Tx may not be Debug
impl<Tx> std::fmt::Debug for ProofTaskManagerHandle<Tx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProofTaskManagerHandle")
            .field("executor", &self.executor)
            .field("active_handles", &self.active_handles)
            .finish()
    }
}

impl<Tx> ProofTaskManagerHandle<Tx>
where
    Tx: DbTx + Send + 'static,
{
    /// Creates a new [`ProofTaskManagerHandle`].
    pub fn new(
        tx_pool_sender: Sender<Arc<ProofTaskTx<Tx>>>,
        tx_pool_receiver: Receiver<Arc<ProofTaskTx<Tx>>>,
        pool_notify: Arc<Notify>,
        executor: Handle,
        active_handles: Arc<AtomicUsize>,
        #[cfg(feature = "metrics")] metrics: Arc<ProofTaskMetrics>,
    ) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self {
            tx_pool_sender,
            tx_pool_receiver,
            pool_notify,
            executor,
            active_handles,
            #[cfg(feature = "metrics")]
            metrics,
        }
    }

    /// Queues a task by checking out a transaction from the pool and spawning it
    /// directly in Tokio's blocking threadpool.
    pub fn queue_task(&self, task: ProofTaskKind) {
        let tx_pool_receiver = self.tx_pool_receiver.clone();
        let tx_pool_sender = self.tx_pool_sender.clone();
        let executor = self.executor.clone();
        let pool_notify = Arc::clone(&self.pool_notify);

        #[cfg(feature = "metrics")]
        let metrics = Arc::clone(&self.metrics);

        // Track metrics for blinded node requests
        #[cfg(feature = "metrics")]
        match &task {
            ProofTaskKind::BlindedAccountNode(_, _) => {
                metrics.account_nodes.fetch_add(1, Ordering::Relaxed);
            }
            ProofTaskKind::BlindedStorageNode(_, _, _) => {
                metrics.storage_nodes.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        self.executor.spawn(async move {
            // Wait asynchronously until a transaction becomes available.
            let proof_tx = loop {
                match tx_pool_receiver.try_recv() {
                    Ok(tx) => break tx,
                    Err(TryRecvError::Empty) => {
                        pool_notify.notified().await;
                    }
                    Err(TryRecvError::Disconnected) => {
                        error!(target: "trie::proof_task", "Transaction pool closed");
                        return;
                    }
                }
            };

            // Execute task in blocking threadpool
            let result = executor
                .spawn_blocking(move || {
                    match task {
                        ProofTaskKind::StorageProof(input, sender) => {
                            proof_tx.storage_proof(input, &sender);
                        }
                        ProofTaskKind::BlindedAccountNode(path, sender) => {
                            proof_tx.blinded_account_node(&path, &sender);
                        }
                        ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                            proof_tx.blinded_storage_node(&account, &path, &sender);
                        }
                    }
                    proof_tx
                })
                .await;

            // Return transaction to pool
            match result {
                Ok(proof_tx) => {
                    match tx_pool_sender.try_send(proof_tx) {
                        Ok(()) => {
                            pool_notify.notify_one();
                        }
                        Err(TrySendError::Full(tx)) => {
                            // Should never happen - we're returning what we took
                            error!(target: "trie::proof_task",
                                "Pool full on return. This should not happen.");

                            // Fallback: Use spawn_blocking to retry the send operation
                            // This prevents losing the transaction from the pool
                            // The send() call blocks a blocking-pool thread, NOT the async worker
                            let tx_pool_sender = tx_pool_sender.clone();
                            let pool_notify = Arc::clone(&pool_notify);
                            executor.spawn_blocking(move || {
                                // Retry the send in a blocking context
                                if tx_pool_sender.send(tx).is_ok() {
                                    pool_notify.notify_one();
                                } else {
                                    error!(target: "trie::proof_task",
                                        "Failed to return transaction to pool even after blocking retry");
                                }
                            });
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            // Pool closed, ignore
                        }
                    }
                }
                Err(e) => {
                    error!(target: "trie::proof_task", ?e, "Proof task panicked, transaction lost from pool");
                }
            }
        });
    }
}

impl<Tx> Clone for ProofTaskManagerHandle<Tx>
where
    Tx: DbTx + Send + 'static,
{
    fn clone(&self) -> Self {
        Self::new(
            self.tx_pool_sender.clone(),
            self.tx_pool_receiver.clone(),
            Arc::clone(&self.pool_notify),
            self.executor.clone(),
            Arc::clone(&self.active_handles),
            #[cfg(feature = "metrics")]
            Arc::clone(&self.metrics),
        )
    }
}

impl<Tx> Drop for ProofTaskManagerHandle<Tx> {
    fn drop(&mut self) {
        // Wake any tasks waiting on a transaction so they can observe shutdown.
        self.pool_notify.notify_waiters();

        // Record metrics if this is the last handle
        if self.active_handles.fetch_sub(1, Ordering::SeqCst) == 1 {
            #[cfg(feature = "metrics")]
            self.metrics.record();
        }
    }
}

impl<Tx> TrieNodeProviderFactory for ProofTaskManagerHandle<Tx>
where
    Tx: DbTx + Send + 'static,
{
    type AccountNodeProvider = ProofTaskTrieNodeProvider<Tx>;
    type StorageNodeProvider = ProofTaskTrieNodeProvider<Tx>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { handle: self.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, handle: self.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
pub enum ProofTaskTrieNodeProvider<Tx> {
    /// Blinded account trie node provider.
    AccountNode {
        /// Handle to the transaction pool
        handle: ProofTaskManagerHandle<Tx>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Handle to the transaction pool
        handle: ProofTaskManagerHandle<Tx>,
    },
}

impl<Tx> std::fmt::Debug for ProofTaskTrieNodeProvider<Tx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountNode { .. } => f.debug_struct("AccountNode").finish(),
            Self::StorageNode { account, .. } => {
                f.debug_struct("StorageNode").field("account", account).finish()
            }
        }
    }
}

impl<Tx> TrieNodeProvider for ProofTaskTrieNodeProvider<Tx>
where
    Tx: DbTx + Send + 'static,
{
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = unbounded();
        match self {
            Self::AccountNode { handle } => {
                handle.queue_task(ProofTaskKind::BlindedAccountNode(*path, tx));
            }
            Self::StorageNode { handle, account } => {
                handle.queue_task(ProofTaskKind::BlindedStorageNode(*account, *path, tx));
            }
        }

        rx.recv().unwrap()
    }
}
