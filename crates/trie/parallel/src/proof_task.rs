//! A Task that manages sending storage multiproof requests to a number of tasks that have
//! longer-running database transactions.
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
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderResult, StateCommitmentProvider,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::StorageProof,
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdatesSorted, HashedPostStateSorted,
    StorageMultiProof,
};
use reth_trie_common::prefix_set::{PrefixSet, PrefixSetMut};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::debug;

type StorageProofResult = Result<StorageMultiProof, ParallelStateRootError>;

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
    /// Storage proof input pending execution
    pending_proofs: VecDeque<(StorageProofInput, Sender<StorageProofResult>)>,
    /// The underlying handle from which to spawn proof tasks
    executor: Handle,
    /// The proof task transactions, containing owned cursor factories that are reused for proof
    /// calculation.
    proof_task_txs: Vec<ProofTaskTx<FactoryTx<Factory>>>,
    /// A receiver for new proof tasks.
    proof_task_rx: Receiver<ProofTaskMessage<FactoryTx<Factory>>>,
    /// A sender for sending back transactions.
    tx_sender: Sender<ProofTaskMessage<FactoryTx<Factory>>>,
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
            pending_proofs: VecDeque::new(),
            executor,
            proof_task_txs: Vec::new(),
            proof_task_rx,
            tx_sender,
        }
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
    pub fn handle(&self) -> ProofTaskManagerHandle<FactoryTx<Factory>> {
        ProofTaskManagerHandle::new(self.tx_sender.clone())
    }
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + 'static,
{
    /// Inserts the storage proof input and sender into the pending proof queue.
    pub fn queue_proof_task(
        &mut self,
        input: StorageProofInput,
        sender: Sender<StorageProofResult>,
    ) {
        self.pending_proofs.push_back((input, sender));
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
        let Some((pending_proof, sender)) = self.pending_proofs.pop_front() else { return Ok(()) };

        let Some(proof_task_tx) = self.get_or_create_tx()? else {
            // if there are no txs available, requeue the proof task
            self.pending_proofs.push_front((pending_proof, sender));
            return Ok(())
        };

        let tx_sender = self.tx_sender.clone();
        self.executor.spawn_blocking(move || {
            proof_task_tx.storage_proof(pending_proof, sender, tx_sender);
        });

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            let message = match self.proof_task_rx.recv() {
                Ok(message) => match message {
                    ProofTaskMessage::StorageProof(input) => Some(input),
                    ProofTaskMessage::Transaction(tx) => {
                        // return the transaction to the pool
                        self.proof_task_txs.push(tx);
                        None
                    }
                    ProofTaskMessage::Terminate => return Ok(()),
                },
                // All senders are disconnected, so we can terminate
                // However this should never happen, as this struct stores a sender
                Err(_) => return Ok(()),
            };

            if let Some((input, sender)) = message {
                self.queue_proof_task(input, sender);
            }

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
    pub const fn new(tx: Tx, task_ctx: ProofTaskCtx) -> Self {
        Self { tx, task_ctx }
    }
}

impl<Tx> ProofTaskTx<Tx>
where
    Tx: DbTx,
{
    /// Calcultes a storage proof for the given hashed address, and desired prefix set.
    pub fn storage_proof(
        self,
        input: StorageProofInput,
        result_sender: Sender<StorageProofResult>,
        tx_sender: Sender<ProofTaskMessage<Tx>>,
    ) {
        debug!(
            target: "trie::parallel_proof",
            hashed_address=?input.hashed_address,
            "Starting storage proof task calculation"
        );

        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(&self.tx),
            &self.task_ctx.nodes_sorted,
        );

        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(&self.tx),
            &self.task_ctx.state_sorted,
        );

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
            target: "trie::parallel_proof",
            hashed_address=?input.hashed_address,
            prefix_set = ?input.prefix_set.len(),
            target_slots = ?target_slots_len,
            proof_time = ?proof_start.elapsed(),
            "Completed storage proof task calculation"
        );

        // send the result back
        if let Err(e) = result_sender.send(result) {
            debug!(
                target: "trie::parallel_proof",
                hashed_address=?input.hashed_address,
                error = ?e,
                task_time = ?proof_start.elapsed(),
                "Failed to send proof result"
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
}

impl ProofTaskCtx {
    /// Creates a new [`ProofTaskCtx`] with the given sorted nodes and state.
    pub const fn new(
        nodes_sorted: Arc<TrieUpdatesSorted>,
        state_sorted: Arc<HashedPostStateSorted>,
    ) -> Self {
        Self { nodes_sorted, state_sorted }
    }
}

/// A message used to send a storage proof request to the [`ProofTaskManager`].
#[derive(Debug)]
pub enum ProofTaskMessage<Tx> {
    /// A storage proof request.
    StorageProof((StorageProofInput, Sender<StorageProofResult>)),
    /// A returned database transaction.
    Transaction(ProofTaskTx<Tx>),
    /// A request to terminate the proof task manager.
    Terminate,
}

/// A handle that wraps a single proof task sender that sends a terminate message on `Drop`.
#[derive(Debug)]
pub struct ProofTaskManagerHandle<Tx> {
    /// The sender for the proof task manager.
    sender: Sender<ProofTaskMessage<Tx>>,
}

impl<Tx> ProofTaskManagerHandle<Tx> {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub const fn new(sender: Sender<ProofTaskMessage<Tx>>) -> Self {
        Self { sender }
    }

    /// Clones and returns the inner sender.
    pub fn sender(&self) -> Sender<ProofTaskMessage<Tx>> {
        self.sender.clone()
    }

    /// Sends a terminate message to the proof task manager.
    pub fn terminate(&self) {
        let _ = self.sender.send(ProofTaskMessage::Terminate);
    }
}

impl<Tx> Drop for ProofTaskManagerHandle<Tx> {
    fn drop(&mut self) {
        self.terminate();
    }
}
