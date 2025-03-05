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
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderResult,
    StateCommitmentProvider,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::StorageProof,
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdatesSorted, HashedPostStateSorted,
    StorageMultiProof,
};
use reth_trie_common::prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSetsMut};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{channel, Receiver, TryRecvError},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::debug;

/// A task that manages sending multiproof requests to a number of tasks that have longer-running
/// database transactions
#[derive(Debug)]
pub struct ProofTaskManager<Tx> {
    /// Storage proof input pending execution
    pending_targets: VecDeque<StorageProofInput>,
    /// The underlying handle from which to spawn proof tasks
    executor: Handle,
    /// The proof task transactions, containing owned cursor factories that are reused for proof
    /// calculation.
    proof_task_txs: Vec<ProofTaskTx<Tx>>,
    /// The currently running proof tasks.
    running_proof_tasks: Vec<Receiver<ProofTaskOutput<Tx>>>,
    /// A receiver for new proof tasks.
    proof_task_rx: Receiver<ProofTaskMessage>,
}

impl<Tx> ProofTaskManager<Tx> {
    /// Creates a new [`ProofTaskManager`] with the given max concurrency, creating that number of
    /// cursor factories.
    pub fn new<Factory>(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        max_concurrency: usize,
        proof_task_rx: Receiver<ProofTaskMessage>,
    ) -> ProviderResult<ProofTaskManager<impl DbTx>>
    where
        Factory: DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + 'static,
    {
        // initialize proof task txs
        let mut proof_task_txs = Vec::with_capacity(max_concurrency);
        for item in &mut proof_task_txs {
            let provider_ro = view.provider_ro()?;
            let tx = provider_ro.into_tx();
            *item = ProofTaskTx::new(tx, task_ctx.clone());
        }

        Ok(ProofTaskManager {
            pending_targets: VecDeque::new(),
            executor,
            proof_task_txs,
            running_proof_tasks: Vec::new(),
            proof_task_rx,
        })
    }
}

impl<Tx> ProofTaskManager<Tx>
where
    Tx: DbTx + 'static,
{
    /// Spawns the proof task on the executor, with the input multiproof targets.
    ///
    /// If a task cannot be spawned immediately, this will be queued for completion later.
    pub fn queue_proof_task(&mut self, input: StorageProofInput) {
        self.pending_targets.push_back(input);
    }

    /// Spawns the next queued proof task on the executor with the given input, if there are any
    /// transactions available.
    pub fn try_spawn_next(&mut self) {
        if self.pending_targets.front().is_none() {
            return;
        }

        let next_input = self.pending_targets.pop_front().unwrap();

        if let Some(proof_task_tx) = self.proof_task_txs.pop() {
            let (tx, rx) = channel();
            self.executor.spawn_blocking(move || {
                let output = proof_task_tx.storage_proof(next_input);
                let _ = tx.send(output);
            });
            self.running_proof_tasks.push(rx);
        }
    }

    /// Loops, managing the proof tasks, and sending new tasks to the executor.
    pub fn run(mut self) {
        loop {
            let message = match self.proof_task_rx.try_recv() {
                Ok(message) => match message {
                    ProofTaskMessage::StorageProof(input) => Some(input),
                    ProofTaskMessage::Terminate => return,
                },
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            };

            if let Some(input) = message {
                self.queue_proof_task(input);
            }

            // try spawning the next task
            self.try_spawn_next();

            // TODO: handle proof task outputs
            for rx in &self.running_proof_tasks {
                match rx.try_recv() {
                    Ok(ProofTaskOutput { tx, result: _ }) => {
                        // TODO: actually handle
                        self.proof_task_txs.push(tx);
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        // TODO: error
                        todo!()
                    }
                }
            }
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
    /// Calcultes a storage proof for the given hashed address, and desired preficx set.
    pub fn storage_proof(self, input: StorageProofInput) -> ProofTaskOutput<Tx> {
        debug!(
            target: "trie::parallel_proof",
            hashed_address=?input.hashed_address,
            "Starting proof calculation"
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
        // TODO: does this need to ever be false?
        .with_branch_node_masks(true)
        .storage_multiproof(input.target_slots)
        .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        debug!(
            target: "trie::parallel_proof",
            hashed_address=?input.hashed_address,
            prefix_set = ?input.prefix_set.len(),
            target_slots = ?target_slots_len,
            proof_time = ?proof_start.elapsed(),
            "Completed proof calculation"
        );

        ProofTaskOutput { tx: self, result }
    }
}

/// This contains the proof calculation result and transaction that was used in proof calculation,
/// which will be reused.
#[derive(Debug)]
pub struct ProofTaskOutput<Tx> {
    /// The tx to be returned back for later use.
    tx: ProofTaskTx<Tx>,
    /// The result of proof calculation.
    result: Result<StorageMultiProof, ParallelStateRootError>,
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
}

/// Data used for initializing cursor factories that is shared across all storage proof instances.
#[derive(Debug, Clone)]
pub struct ProofTaskCtx {
    /// The sorted collection of cached in-memory intermediate trie nodes that can be reused for
    /// computation.
    nodes_sorted: Arc<TrieUpdatesSorted>,
    /// The sorted in-memory overlay hashed state.
    state_sorted: Arc<HashedPostStateSorted>,

    // TODO: make use of this, since this is also reused across multiproof calls
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `state_sorted` might be present here,
    /// if we have cached nodes for them.
    prefix_sets: Arc<TriePrefixSetsMut>,
}

/// A message used to send a storage proof request to the [`ProofTaskManager`].
#[derive(Debug)]
pub enum ProofTaskMessage {
    /// A storage proof request.
    StorageProof(StorageProofInput),
    /// A request to terminate the proof task manager.
    Terminate,
}
