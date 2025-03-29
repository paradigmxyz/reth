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
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult, StateCommitmentProvider,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::TriePrefixSetsMut,
    proof::{ProofBlindedProviderFactory, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    HashedPostStateSorted, Nibbles, StorageMultiProof,
};
use reth_trie_common::prefix_set::{PrefixSet, PrefixSetMut};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_sparse::blinded::{BlindedProvider, BlindedProviderFactory, RevealedNode};
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
use tracing::debug;

type StorageProofResult = Result<StorageMultiProof, ParallelStateRootError>;
type BlindedNodeResult = Result<Option<RevealedNode>, SparseTrieError>;

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
        }
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
    pub fn handle(&self) -> ProofTaskManagerHandle<FactoryTx<Factory>> {
        ProofTaskManagerHandle::new(self.tx_sender.clone(), self.active_handles.clone())
    }
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + 'static,
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
            return Ok(Some(ProofTaskTx::new(tx, self.task_ctx.clone())));
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
        });

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => match message {
                    ProofTaskMessage::QueueTask(task) => {
                        // queue the task
                        self.queue_proof_task(task)
                    }
                    ProofTaskMessage::Transaction(tx) => {
                        // return the transaction to the pool
                        self.proof_task_txs.push(tx);
                    }
                    ProofTaskMessage::Terminate => return Ok(()),
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
}

impl<Tx> ProofTaskTx<Tx> {
    /// Initializes a [`ProofTaskTx`] using the given transaction anda[`ProofTaskCtx`].
    const fn new(tx: Tx, task_ctx: ProofTaskCtx) -> Self {
        Self { tx, task_ctx }
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
        debug!(
            target: "trie::proof_task",
            hashed_address=?input.hashed_address,
            "Starting storage proof task calculation"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let target_slots_len = input.target_slots.len();
        let proof_start = Instant::now();
        let result = StorageProof::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            input.hashed_address,
        )
        .with_prefix_set_mut(PrefixSetMut::from(input.prefix_set.iter().cloned()))
        .with_branch_node_masks(input.with_branch_node_masks)
        .storage_multiproof(input.target_slots)
        .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        debug!(
            target: "trie::proof_task",
            hashed_address=?input.hashed_address,
            prefix_set = ?input.prefix_set.len(),
            target_slots = ?target_slots_len,
            proof_time = ?proof_start.elapsed(),
            "Completed storage proof task calculation"
        );

        // send the result back
        if let Err(error) = result_sender.send(result) {
            debug!(
                target: "trie::proof_task",
                hashed_address = ?input.hashed_address,
                ?error,
                task_time = ?proof_start.elapsed(),
                "Failed to send proof result"
            );
        }

        // send the tx back
        let _ = tx_sender.send(ProofTaskMessage::Transaction(self));
    }

    /// Retrieves blinded account node by path.
    fn blinded_account_node(
        self,
        path: Nibbles,
        result_sender: Sender<BlindedNodeResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        debug!(
            target: "trie::proof_task",
            ?path,
            "Starting blinded account node retrieval"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let blinded_provider_factory = ProofBlindedProviderFactory::new(
            trie_cursor_factory,
            hashed_cursor_factory,
            self.task_ctx.prefix_sets.clone(),
        );

        let start = Instant::now();
        let result = blinded_provider_factory.account_node_provider().blinded_node(&path);
        debug!(
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
        result_sender: Sender<BlindedNodeResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        debug!(
            target: "trie::proof_task",
            ?account,
            ?path,
            "Starting blinded storage node retrieval"
        );

        let (trie_cursor_factory, hashed_cursor_factory) = self.create_factories();

        let blinded_provider_factory = ProofBlindedProviderFactory::new(
            trie_cursor_factory,
            hashed_cursor_factory,
            self.task_ctx.prefix_sets.clone(),
        );

        let start = Instant::now();
        let result = blinded_provider_factory.storage_node_provider(account).blinded_node(&path);
        debug!(
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
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given hashed address, prefix set, and target
    /// slots.
    pub const fn new(
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: B256Set,
        with_branch_node_masks: bool,
    ) -> Self {
        Self { hashed_address, prefix_set, target_slots, with_branch_node_masks }
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
    BlindedAccountNode(Nibbles, Sender<BlindedNodeResult>),
    /// A blinded storage node request.
    BlindedStorageNode(B256, Nibbles, Sender<BlindedNodeResult>),
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

impl<Tx: DbTx> BlindedProviderFactory for ProofTaskManagerHandle<Tx> {
    type AccountNodeProvider = ProofTaskBlindedNodeProvider<Tx>;
    type StorageNodeProvider = ProofTaskBlindedNodeProvider<Tx>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskBlindedNodeProvider::AccountNode { sender: self.sender.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskBlindedNodeProvider::StorageNode { account, sender: self.sender.clone() }
    }
}

/// Blinded node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskBlindedNodeProvider<Tx> {
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

impl<Tx: DbTx> BlindedProvider for ProofTaskBlindedNodeProvider<Tx> {
    fn blinded_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = channel();
        match self {
            Self::AccountNode { sender } => {
                let _ = sender.send(ProofTaskMessage::QueueTask(
                    ProofTaskKind::BlindedAccountNode(path.clone(), tx),
                ));
            }
            Self::StorageNode { sender, account } => {
                let _ = sender.send(ProofTaskMessage::QueueTask(
                    ProofTaskKind::BlindedStorageNode(*account, path.clone(), tx),
                ));
            }
        }

        rx.recv().unwrap()
    }
}
