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
use crossbeam_channel::{unbounded, Receiver, SendError, Sender};
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
    collections::VecDeque,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};
use tracing::{debug, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Thread pool where each worker maintains its own database transaction
#[derive(Debug)]
struct ProofThreadPool<Factory> {
    work_sender: Sender<ProofTaskKind>,
    _phantom: PhantomData<Factory>,
}

/// Thread pool with self-recovering workers, each maintaining its own database transaction.
///
/// Workers automatically respawn on panic with fresh transactions.
impl<Factory> ProofThreadPool<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Send + Sync + Clone + 'static,
{
    fn new(
        num_workers: usize,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
    ) -> ProviderResult<Self> {
        let (work_sender, work_receiver) = unbounded();

        for worker_id in 0..num_workers {
            Self::spawn_worker(
                worker_id,
                view.clone(),
                task_ctx.clone(),
                work_receiver.clone(),
            )?;
        }

        Ok(Self { work_sender, _phantom: PhantomData })
    }

    /// Spawns a resilient worker that will respawn itself on panic
    fn spawn_worker(
        worker_id: usize,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        work_receiver: Receiver<ProofTaskKind>,
    ) -> ProviderResult<()> {
        thread::spawn(move || {
            loop {
                let provider_ro = match view.provider_ro() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!(
                            target: "trie::proof_task",
                            worker_id,
                            error = ?e,
                            "Failed to create provider for worker, supervisor exiting"
                        );
                        break;
                    }
                };

                let tx = provider_ro.into_tx();
                let proof_task_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id);
                let receiver = work_receiver.clone();

                let worker_handle = thread::spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        match task {
                            ProofTaskKind::StorageProof(input, result_sender) => {
                                proof_task_tx.storage_proof(input, result_sender);
                            }
                            ProofTaskKind::BlindedAccountNode(path, result_sender) => {
                                proof_task_tx.blinded_account_node(path, result_sender);
                            }
                            ProofTaskKind::BlindedStorageNode(account, path, result_sender) => {
                                proof_task_tx.blinded_storage_node(account, path, result_sender);
                            }
                        }
                    }
                });

                match worker_handle.join() {
                    Ok(_) => {
                        debug!(
                            target: "trie::proof_task",
                            worker_id,
                            "Worker terminated normally"
                        );
                        break;
                    }
                    Err(panic_payload) => {
                        tracing::warn!(
                            target: "trie::proof_task",
                            worker_id,
                            panic = ?panic_payload,
                            "Worker panicked, respawning with fresh transaction"
                        );
                    }
                }
            }
        });

        Ok(())
    }

    fn dispatch_task(&self, task: ProofTaskKind) -> Result<(), SendError<ProofTaskKind>> {
        self.work_sender.send(task)
    }
}

/// A task that manages sending multiproof requests to a number of tasks that have longer-running
/// database transactions
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    /// Max number of worker threads to create
    max_concurrency: usize,
    /// Consistent view provider used for creating transactions on-demand
    view: ConsistentDbView<Factory>,
    /// Proof task context shared across all proof tasks
    task_ctx: ProofTaskCtx,
    /// Proof tasks pending execution
    pending_tasks: VecDeque<ProofTaskKind>,
    /// A receiver for new proof tasks.
    proof_task_rx: Receiver<ProofTaskMessage>,
    /// A sender for control messages.
    control_sender: Sender<ProofTaskMessage>,
    /// The number of active handles.
    ///
    /// Incremented in [`ProofTaskManagerHandle::new`] and decremented in
    /// [`ProofTaskManagerHandle::drop`].
    active_handles: Arc<AtomicUsize>,
    /// Thread pool where each worker maintains its own database transaction
    thread_pool: Option<ProofThreadPool<Factory>>,
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
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        max_concurrency: usize,
    ) -> Self {
        let (control_sender, proof_task_rx) = unbounded();
        Self {
            max_concurrency,
            view,
            task_ctx,
            pending_tasks: VecDeque::new(),
            proof_task_rx,
            control_sender,
            active_handles: Arc::new(AtomicUsize::new(0)),
            thread_pool: None,
            #[cfg(feature = "metrics")]
            metrics: ProofTaskMetrics::default(),
        }
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
    pub fn handle(&self) -> ProofTaskManagerHandle {
        ProofTaskManagerHandle::new(self.control_sender.clone(), self.active_handles.clone())
    }
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Send + Sync + Clone + 'static,
{
    /// Inserts the task into the pending tasks queue.
    pub fn queue_proof_task(&mut self, task: ProofTaskKind) {
        self.pending_tasks.push_back(task);
    }

    /// Gets or creates the thread pool
    fn get_or_create_thread_pool(&mut self) -> ProviderResult<&ProofThreadPool<Factory>> {
        if self.thread_pool.is_none() {
            let thread_pool = ProofThreadPool::new(
                self.max_concurrency,
                self.view.clone(),
                self.task_ctx.clone(),
            )?;
            self.thread_pool = Some(thread_pool);
        }
        Ok(self.thread_pool.as_ref().unwrap())
    }

    /// Spawns the next queued proof task on the thread pool with the given input.
    ///
    /// This will return an error if the thread pool must be created on-demand and the consistent
    /// view provider fails.
    pub fn try_spawn_next(&mut self) -> ProviderResult<()> {
        let Some(task) = self.pending_tasks.pop_front() else { return Ok(()) };

        // Get or create the thread pool
        let thread_pool = self.get_or_create_thread_pool()?;

        // Dispatch the task to the thread pool
        if let Err(send_error) = thread_pool.dispatch_task(task) {
            // If dispatch fails, requeue the task that failed to send
            self.pending_tasks.push_front(send_error.0);
        }

        Ok(())
    }

    /// Loops, managing the proof tasks, and sending new tasks to the thread pool.
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
    fn storage_proof(&self, input: StorageProofInput, result_sender: Sender<StorageProofResult>) {
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
    }

    /// Retrieves blinded account node by path.
    fn blinded_account_node(&self, path: Nibbles, result_sender: Sender<TrieNodeProviderResult>) {
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
    }

    /// Retrieves blinded storage node of the given account by path.
    fn blinded_storage_node(
        &self,
        account: B256,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
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
pub enum ProofTaskMessage {
    /// A request to queue a proof task.
    QueueTask(ProofTaskKind),
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
pub struct ProofTaskManagerHandle {
    /// The sender for the proof task manager.
    sender: Sender<ProofTaskMessage>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
}

impl ProofTaskManagerHandle {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub fn new(sender: Sender<ProofTaskMessage>, active_handles: Arc<AtomicUsize>) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self { sender, active_handles }
    }

    /// Queues a task to the proof task manager.
    pub fn queue_task(&self, task: ProofTaskKind) -> Result<(), SendError<ProofTaskMessage>> {
        self.sender.send(ProofTaskMessage::QueueTask(task))
    }

    /// Terminates the proof task manager.
    pub fn terminate(&self) {
        let _ = self.sender.send(ProofTaskMessage::Terminate);
    }
}

impl Clone for ProofTaskManagerHandle {
    fn clone(&self) -> Self {
        Self::new(self.sender.clone(), self.active_handles.clone())
    }
}

impl Drop for ProofTaskManagerHandle {
    fn drop(&mut self) {
        // Decrement the number of active handles and terminate the manager if it was the last
        // handle.
        if self.active_handles.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.terminate();
        }
    }
}

impl TrieNodeProviderFactory for ProofTaskManagerHandle {
    type AccountNodeProvider = ProofTaskTrieNodeProvider;
    type StorageNodeProvider = ProofTaskTrieNodeProvider;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { sender: self.sender.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, sender: self.sender.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider {
    /// Blinded account trie node provider.
    AccountNode {
        /// Sender to the proof task.
        sender: Sender<ProofTaskMessage>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Sender to the proof task.
        sender: Sender<ProofTaskMessage>,
    },
}

impl TrieNodeProvider for ProofTaskTrieNodeProvider {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = unbounded();
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
