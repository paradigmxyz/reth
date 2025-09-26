//! A Task that manages sending proof requests to a number of tasks that have longer-running
//! database transactions.
//!
//! The [`ProofTaskManager`] ensures that there are a max number of currently executing proof tasks,
//! and is responsible for managing the fixed number of database transactions created at the start
//! of the task.
//!
//! Individual [`ProofTaskTx`] instances manage a dedicated [`InMemoryTrieCursorFactory`] and
//! [`HashedPostStateCursorFactory`], which are each backed by a database transaction.

use crate::{root::ParallelStateRootError, storage_root_targets::StorageRootTargets};
use alloy_primitives::{map::B256Set, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSetsMut,
    proof::{ProofTrieNodeProviderFactory, StorageProof},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    DecodedMultiProof, DecodedStorageMultiProof, HashedPostStateSorted, MultiProofTargets, Nibbles,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
    proof::DecodedProofNodes,
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
type AccountProofResult = Result<DecodedMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// A task that manages sending multiproof requests to a number of tasks that have longer-running
/// database transactions
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    /// Max number of database transactions to create
    max_concurrency: usize,
    /// Number of database transactions created
    total_transactions: usize,
    /// Consistent view provider used for creating transactions on-demand
    view: ConsistentDbView<Factory>,
    /// Proof task context shared across all proof tasks
    task_ctx: ProofTaskCtx,
    /// Proof tasks pending execution
    pending_tasks: VecDeque<ProofTaskKind>,
    /// The underlying handle from which to spawn proof tasks
    executor: Handle,
    /// The proof task transactions, containing owned cursor factories that are reused for proof
    /// calculation.
    proof_task_txs: Vec<ProofTaskTx<FactoryTx<Factory>>>,
    /// A receiver for new proof tasks.
    proof_task_rx: Receiver<ProofTaskMessage<FactoryTx<Factory>>>,
    /// A sender for sending back transactions.
    tx_sender: Sender<ProofTaskMessage<FactoryTx<Factory>>>,
    /// The number of active handles.
    ///
    /// Incremented in [`ProofTaskManagerHandle::new`] and decremented in
    /// [`ProofTaskManagerHandle::drop`].
    active_handles: Arc<AtomicUsize>,
    /// Metrics tracking blinded node fetches.
    #[cfg(feature = "metrics")]
    metrics: ProofTaskMetrics,
}

impl<Factory: DatabaseProviderFactory> ProofTaskManager<Factory> {
    /// Creates a new [`ProofTaskManager`] with the given max concurrency, creating that number of
    /// cursor factories.
    ///
    /// Returns an error if the consistent view provider fails to create a read-only transaction.
    pub fn new(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        max_concurrency: usize,
    ) -> Self {
        let (tx_sender, proof_task_rx) = channel();
        Self {
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
            metrics: ProofTaskMetrics::default(),
        }
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
        let active_handles = self.active_handles.clone();
        self.executor.spawn_blocking(move || match task {
            ProofTaskKind::StorageProof(input, sender) => {
                proof_task_tx.storage_proof(input, sender, tx_sender);
            }
            ProofTaskKind::AccountProof(input, sender) => {
                let handle = ProofTaskManagerHandle::new(tx_sender.clone(), active_handles.clone());
                proof_task_tx.account_proof(input, sender, tx_sender, handle);
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
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => match message {
                    ProofTaskMessage::QueueTask(task) => {
                        // Track metrics for blinded node requests
                        #[cfg(feature = "metrics")]
                        match &task {
                            ProofTaskKind::BlindedAccountNode(_, _) => {
                                self.metrics.account_nodes += 1;
                            }
                            ProofTaskKind::BlindedStorageNode(_, _, _) => {
                                self.metrics.storage_nodes += 1;
                            }
                            _ => {}
                        }
                        // queue the task
                        self.queue_proof_task(task)
                    }
                    ProofTaskMessage::Transaction(tx) => {
                        // return the transaction to the pool
                        self.proof_task_txs.push(tx);
                    }
                    ProofTaskMessage::Terminate => {
                        // Record metrics before terminating
                        #[cfg(feature = "metrics")]
                        self.metrics.record();
                        return Ok(())
                    }
                },
                // All senders are disconnected, so we can terminate
                // However this should never happen, as this struct stores a sender
                Err(_) => return Ok(()),
            };

            // try spawning the next task
            self.try_spawn_next()?;
        }
    }
}

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
    fn create_factories(
        &self,
    ) -> (
        InMemoryTrieCursorFactory<'_, DatabaseTrieCursorFactory<'_, Tx>>,
        HashedPostStateCursorFactory<'_, DatabaseHashedCursorFactory<'_, Tx>>,
    ) {
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(&self.tx),
            &self.task_ctx.nodes_sorted,
        );

        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(&self.tx),
            &self.task_ctx.state_sorted,
        );

        (trie_cursor_factory, hashed_cursor_factory)
    }

    /// Calculates a storage proof for the given hashed address, and desired prefix set.
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

    /// Calculates an account multiproof for the given targets, reusing this tx and delegating
    /// storage proofs back to the task manager.
    fn account_proof(
        self,
        input: AccountProofInput,
        result_sender: Sender<AccountProofResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
        storage_proof_task_handle: ProofTaskManagerHandle<Tx>,
    ) {
        trace!(
            target: "trie::proof_task",
            targets = ?input.targets.len(),
            branch_masks = input.with_branch_node_masks,
            "Starting account multiproof task calculation",
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        // Extend prefix sets with targets
        let mut prefix_sets = (*self.task_ctx.prefix_sets).clone();
        prefix_sets.extend(TriePrefixSetsMut {
            account_prefix_set: PrefixSetMut::from(
                input.targets.keys().copied().map(reth_trie::Nibbles::unpack),
            ),
            storage_prefix_sets: input
                .targets
                .iter()
                .filter(|&(_hashed_address, slots)| !slots.is_empty())
                .map(|(hashed_address, slots)| {
                    (
                        *hashed_address,
                        PrefixSetMut::from(slots.iter().map(reth_trie::Nibbles::unpack)),
                    )
                })
                .collect(),
            destroyed_accounts: Default::default(),
        });
        let prefix_sets = prefix_sets.freeze();

        let storage_root_targets = StorageRootTargets::new(
            prefix_sets.account_prefix_set.iter().map(|nibbles| B256::from_slice(&nibbles.pack())),
            prefix_sets.storage_prefix_sets.clone(),
        );

        // stores the receiver for the storage proof outcome for the hashed addresses
        let mut storage_proof_receivers = alloy_primitives::map::B256Map::with_capacity_and_hasher(
            storage_root_targets.len(),
            Default::default(),
        );

        for (hashed_address, prefix_set) in storage_root_targets {
            let target_slots = input.targets.get(&hashed_address).cloned().unwrap_or_default();

            let storage_input = StorageProofInput::new(
                hashed_address,
                prefix_set,
                target_slots,
                input.with_branch_node_masks,
                input.multi_added_removed_keys.clone(),
            );

            let (sender, receiver) = std::sync::mpsc::channel();
            let _ = storage_proof_task_handle
                .queue_task(ProofTaskKind::StorageProof(storage_input, sender));
            storage_proof_receivers.insert(hashed_address, receiver);
        }

        let accounts_added_removed_keys =
            input.multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

        // Create the walker.
        let walker = reth_trie::walker::TrieWalker::<_>::state_trie(
            trie_cursor_factory.account_trie_cursor().expect("db cursor"),
            prefix_sets.account_prefix_set,
        )
        .with_added_removed_keys(accounts_added_removed_keys)
        .with_deletions_retained(true);

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer = input
            .targets
            .keys()
            .map(reth_trie::Nibbles::unpack)
            .collect::<reth_trie_common::proof::ProofRetainer>()
            .with_added_removed_keys(accounts_added_removed_keys);
        let mut hash_builder = reth_trie::HashBuilder::default()
            .with_proof_retainer(retainer)
            .with_updates(input.with_branch_node_masks);

        // Initialize all storage multiproofs as empty.
        let mut collected_decoded_storages: alloy_primitives::map::B256Map<
            DecodedStorageMultiProof,
        > = input.targets.keys().map(|key| (*key, DecodedStorageMultiProof::empty())).collect();
        let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
        let mut account_node_iter = TrieNodeIter::state_trie(
            walker,
            hashed_cursor_factory.hashed_account_cursor().expect("db cursor"),
        );

        while let Some(account_node) = account_node_iter.try_next().ok().flatten() {
            match account_node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let decoded_storage_multiproof = match storage_proof_receivers
                        .remove(&hashed_address)
                    {
                        Some(rx) => match rx.recv() {
                            Ok(res) => match res {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = result_sender.send(Err(e));
                                    let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
                                    return;
                                }
                            },
                            Err(e) => {
                                let _ = result_sender.send(Err(ParallelStateRootError::Other(
                                    format!("channel closed for {hashed_address}: {}", e),
                                )));
                                let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
                                return;
                            }
                        },
                        None => {
                            // Fallback to direct calculation using this tx if no receiver present
                            let raw_fallback_proof = StorageProof::new_hashed(
                                trie_cursor_factory.clone(),
                                hashed_cursor_factory.clone(),
                                hashed_address,
                            )
                            .with_prefix_set_mut(Default::default())
                            .storage_multiproof(
                                input.targets.get(&hashed_address).cloned().unwrap_or_default(),
                            )
                            .map_err(|e| ParallelStateRootError::Other(e.to_string()));

                            match raw_fallback_proof.and_then(|p| {
                                p.try_into().map_err(|e: alloy_rlp::Error| {
                                    ParallelStateRootError::Other(e.to_string())
                                })
                            }) {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = result_sender.send(Err(e));
                                    let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
                                    return;
                                }
                            }
                        }
                    };

                    // Encode account
                    account_rlp.clear();
                    let account = account.into_trie_account(decoded_storage_multiproof.root);
                    {
                        use alloy_rlp::Encodable as _;
                        account.encode(&mut account_rlp as &mut dyn alloy_rlp::BufMut);
                    }

                    hash_builder.add_leaf(reth_trie::Nibbles::unpack(hashed_address), &account_rlp);

                    // Only retain storage proofs for target accounts
                    if input.targets.contains_key(&hashed_address) {
                        collected_decoded_storages
                            .insert(hashed_address, decoded_storage_multiproof);
                    }
                }
            }
        }
        let _ = hash_builder.root();

        // Build decoded proof nodes
        let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
        let decoded_account_subtree = match DecodedProofNodes::try_from(account_subtree_raw_nodes) {
            Ok(nodes) => nodes,
            Err(err) => {
                let _ = result_sender.send(Err(ParallelStateRootError::Other(err.to_string())));
                let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
                return;
            }
        };

        let (branch_node_hash_masks, branch_node_tree_masks) = if input.with_branch_node_masks {
            let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
            (
                updated_branch_nodes.iter().map(|(path, node)| (*path, node.hash_mask)).collect(),
                updated_branch_nodes
                    .into_iter()
                    .map(|(path, node)| (path, node.tree_mask))
                    .collect(),
            )
        } else {
            (Default::default(), Default::default())
        };

        let result = DecodedMultiProof {
            account_subtree: decoded_account_subtree,
            branch_node_hash_masks,
            branch_node_tree_masks,
            storages: collected_decoded_storages,
        };

        // send the result back
        let _ = result_sender.send(Ok(result));

        // return the tx to the pool
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

/// Input for an account multiproof task.
#[derive(Debug)]
pub struct AccountProofInput {
    /// The targets to include in the multiproof.
    pub targets: MultiProofTargets,
    /// Whether or not to collect branch node masks.
    pub with_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
}

impl AccountProofInput {
    /// Create new account multiproof input.
    pub const fn new(
        targets: MultiProofTargets,
        with_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    ) -> Self {
        Self { targets, with_branch_node_masks, multi_added_removed_keys }
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
    /// An account multiproof request.
    AccountProof(AccountProofInput, Sender<AccountProofResult>),
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
