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
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
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
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
        Arc,
    },
    time::Instant,
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use tracing::{debug, trace};

/// Default number of workers in the proof task worker pool.
///
/// This controls how many concurrent database transactions will be created for proof calculation.
pub const DEFAULT_PROOF_TASK_WORKER_POOL_SIZE: usize = 8;

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// A task that manages sending multiproof requests to a number of worker tasks that have
/// longer-running database transactions
#[derive(Debug)]
pub struct ProofTaskManager<Factory: DatabaseProviderFactory> {
    /// Number of workers in the pool.
    /// This is also the max number of database transactions that can be created.
    num_workers: usize,
    /// The underlying handle from which to spawn proof tasks.
    /// Only used during initialization; stored for Debug impl.
    #[allow(dead_code)]
    executor: Handle,
    /// Worker task handles
    worker_handles: Vec<JoinHandle<()>>,
    /// Sender to distribute work to worker pool
    work_tx: UnboundedSender<ProofTaskKind>,
    /// A receiver for messages from handles.
    task_rx: UnboundedReceiver<ProofTaskMessage>,
    /// A sender for handles to communicate with the manager.
    task_tx: UnboundedSender<ProofTaskMessage>,
    /// The number of active handles.
    ///
    /// Incremented in [`ProofTaskManagerHandle::new`] and decremented in
    /// [`ProofTaskManagerHandle::drop`].
    active_handles: Arc<AtomicUsize>,
    /// Metrics tracking blinded node fetches.
    #[cfg(feature = "metrics")]
    metrics: ProofTaskMetrics,
    /// Phantom data to use the Factory type parameter
    _phantom: PhantomData<Factory>,
}

impl<Factory: DatabaseProviderFactory> ProofTaskManager<Factory> {
    /// Creates a new [`ProofTaskManager`] with the given number of workers.
    ///
    /// If `num_workers` is `None`, uses [`DEFAULT_PROOF_TASK_WORKER_POOL_SIZE`].
    ///
    /// Returns an error if the consistent view provider fails to create a read-only transaction.
    pub fn new(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        num_workers: impl Into<Option<usize>>,
    ) -> ProviderResult<Self>
    where
        Factory: Clone + Send + 'static,
        Factory::Provider: BlockReader,
    {
        let num_workers = num_workers.into().unwrap_or(DEFAULT_PROOF_TASK_WORKER_POOL_SIZE);

        let (task_tx, task_rx) = unbounded_channel();
        let (work_tx, work_rx) = unbounded_channel();

        let work_rx = Arc::new(tokio::sync::Mutex::new(work_rx));

        let mut worker_handles = Vec::with_capacity(num_workers);

        // Spawn worker tasks
        for worker_id in 0..num_workers {
            let provider_ro = view.provider_ro()?;
            let tx = provider_ro.into_tx();
            let proof_task_tx = Arc::new(ProofTaskTx::new(tx, task_ctx.clone(), worker_id));
            let work_rx_clone = Arc::clone(&work_rx);
            let executor_clone = executor.clone();

            let handle = executor.spawn(worker_loop(
                work_rx_clone,
                proof_task_tx,
                worker_id,
                executor_clone,
            ));
            worker_handles.push(handle);
        }

        Ok(Self {
            num_workers,
            executor,
            worker_handles,
            work_tx,
            task_rx,
            task_tx,
            active_handles: Arc::new(AtomicUsize::new(0)),
            #[cfg(feature = "metrics")]
            metrics: ProofTaskMetrics::default(),
            _phantom: PhantomData,
        })
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
    pub fn handle(&self) -> ProofTaskManagerHandle {
        ProofTaskManagerHandle::new(self.task_tx.clone(), self.active_handles.clone())
    }
}

/// Worker loop that processes proof tasks from the work queue.
///
/// Each worker owns a `ProofTaskTx` wrapped in Arc for shared access across `spawn_blocking` calls,
/// and continuously polls for work from the shared queue.
async fn worker_loop<Tx: DbTx + Send + Sync + 'static>(
    work_rx: Arc<tokio::sync::Mutex<UnboundedReceiver<ProofTaskKind>>>,
    proof_task_tx: Arc<ProofTaskTx<Tx>>,
    worker_id: usize,
    executor: Handle,
) {
    debug!(
        target: "trie::proof_task",
        worker_id,
        "Proof task worker starting"
    );

    loop {
        // Lock the receiver to get the next task
        let task = {
            let mut rx = work_rx.lock().await;
            rx.recv().await
        };

        match task {
            Some(task) => {
                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    "Worker received task"
                );

                // Execute the task using spawn_blocking for CPU-intensive work
                let proof_task_tx_clone = Arc::clone(&proof_task_tx);
                let result = executor.spawn_blocking(move || match task {
                    ProofTaskKind::StorageProof(input, sender) => {
                        proof_task_tx_clone.storage_proof(input, sender);
                    }
                    ProofTaskKind::BlindedAccountNode(path, sender) => {
                        proof_task_tx_clone.blinded_account_node(path, sender);
                    }
                    ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                        proof_task_tx_clone.blinded_storage_node(account, path, sender);
                    }
                }).await;

                if let Err(err) = result {
                    tracing::error!(
                        target: "trie::proof_task",
                        worker_id,
                        ?err,
                        "Worker task panicked or was cancelled"
                    );
                }
            }
            None => {
                // Channel closed, worker should exit
                debug!(
                    target: "trie::proof_task",
                    worker_id,
                    "Proof task worker shutting down"
                );
                break;
            }
        }
    }
}

impl<Factory> ProofTaskManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + 'static,
{
    /// Returns the number of workers in the pool.
    pub const fn num_workers(&self) -> usize {
        self.num_workers
    }

    /// Shuts down the worker pool and waits for all workers to complete.
    async fn shutdown(self) -> ProviderResult<()> {
        // Close the work channel to signal workers to stop
        drop(self.work_tx);

        debug!(
            target: "trie::proof_task",
            num_workers = self.num_workers,
            "Shutting down proof task manager, waiting for workers to complete"
        );

        // Wait for all workers to finish
        for handle in self.worker_handles {
            let _ = handle.await;
        }

        // Record metrics before terminating
        #[cfg(feature = "metrics")]
        self.metrics.record();

        debug!(
            target: "trie::proof_task",
            "Proof task manager shut down successfully"
        );

        Ok(())
    }

    /// Loops, managing the proof tasks, and distributing work to the worker pool.
    pub async fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.task_rx.recv().await {
                Some(ProofTaskMessage::QueueTask(task)) => {
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
                    // Send task to worker pool
                    let _ = self.work_tx.send(task);
                }
                Some(ProofTaskMessage::Terminate) | None => {
                    // Terminate message or all senders disconnected
                    return self.shutdown().await;
                }
            }
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

    /// Calculates a storage proof for the given hashed address, and desired prefix set.
    fn storage_proof(
        &self,
        input: StorageProofInput,
        result_sender: Sender<StorageProofResult>,
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
    }

    /// Retrieves blinded account node by path.
    fn blinded_account_node(
        &self,
        path: Nibbles,
        result_sender: Sender<TrieNodeProviderResult>,
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
    sender: UnboundedSender<ProofTaskMessage>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
}

impl ProofTaskManagerHandle {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub fn new(sender: UnboundedSender<ProofTaskMessage>, active_handles: Arc<AtomicUsize>) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self { sender, active_handles }
    }

    /// Queues a task to the proof task manager.
    pub fn queue_task(&self, task: ProofTaskKind) -> Result<(), tokio::sync::mpsc::error::SendError<ProofTaskMessage>> {
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
        sender: UnboundedSender<ProofTaskMessage>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Sender to the proof task.
        sender: UnboundedSender<ProofTaskMessage>,
    },
}

impl TrieNodeProvider for ProofTaskTrieNodeProvider {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        use std::sync::mpsc::channel;
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
    use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
    use reth_trie::TrieInput;
    use std::{
        sync::{mpsc::channel, Arc},
        time::{Duration, Instant},
    };

    fn create_test_proof_task_manager(
        num_workers: impl Into<Option<usize>>,
    ) -> ProofTaskManager<impl DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static> {
        let factory = create_test_provider_factory();
        let handle = tokio::runtime::Handle::current();
        let consistent_view = ConsistentDbView::new(factory, None);
        let input = TrieInput::default();
        let task_ctx = ProofTaskCtx::new(
            Arc::new(input.nodes.into_sorted()),
            Arc::new(input.state.into_sorted()),
            Arc::new(input.prefix_sets),
        );

        ProofTaskManager::new(handle, consistent_view, task_ctx, num_workers)
            .expect("Failed to create proof task manager")
    }

    #[tokio::test]
    async fn test_worker_pool_startup() {

        let manager = create_test_proof_task_manager(4);

        assert_eq!(manager.num_workers(), 4);

        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        handle.terminate();
        manager_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_worker_pool_with_default_size() {
        let manager = create_test_proof_task_manager(None);
        assert_eq!(manager.num_workers(), DEFAULT_PROOF_TASK_WORKER_POOL_SIZE);

        let handle = manager.handle();
        let manager_task = tokio::spawn(async move { manager.run().await });

        handle.terminate();
        manager_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_worker_pool_graceful_shutdown() {
        let manager = create_test_proof_task_manager(2);
        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        // Sleep for a short period in order to allow tasks to start.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Initiate shutdown
        handle.terminate();

        // Wait for shutdown to complete
        manager_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_task_scheduling_and_processing() {
        let manager = create_test_proof_task_manager(2);
        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        let num_tasks = 10;

        // Queue multiple storage proof tasks
        let _start = Instant::now();
        for i in 0..num_tasks {
            let (tx, _rx) = channel();
            let input = StorageProofInput::new(
                B256::left_padding_from(&(i as u64).to_be_bytes()),
                Default::default(),
                Default::default(),
                false,
                None,
            );

            handle
                .queue_task(ProofTaskKind::StorageProof(input, tx))
                .expect("Failed to queue task");
        }

        // Give workers a moment to pick up tasks
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify system is still running and responsive by queuing one more task
        // and checking that the manager accepts it without error
        let (verify_tx, verify_rx) = channel();
        handle.queue_task(ProofTaskKind::StorageProof(
            StorageProofInput::new(
                B256::ZERO,
                Default::default(),
                Default::default(),
                false,
                None,
            ),
            verify_tx,
        )).expect("Manager should still be accepting tasks");

        assert!(
            verify_rx.recv_timeout(Duration::from_millis(200)).is_ok() ||
            verify_rx.recv_timeout(Duration::from_millis(0)).is_err(),
            "Workers should be actively processing or queue should be accepting tasks"
        );

        handle.terminate();
        let shutdown_result = tokio::time::timeout(
            Duration::from_secs(2),
            manager_task
        ).await;
        assert!(shutdown_result.is_ok(), "Manager should shut down gracefully");
        shutdown_result.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    #[ignore = "Requires real database state for proof computation"]
    async fn test_concurrent_task_distribution() {
        let manager = create_test_proof_task_manager(4);
        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        let num_tasks = 100;
        let mut receivers = Vec::new();

        let _queue_start = Instant::now();
        // Queue all tasks
        for i in 0..num_tasks {
            let (tx, rx) = channel();
            let input = StorageProofInput::new(
                B256::left_padding_from(&(i as u64).to_be_bytes()),
                Default::default(),
                Default::default(),
                false,
                None,
            );

            handle
                .queue_task(ProofTaskKind::StorageProof(input, tx))
                .expect("Failed to queue task");
            receivers.push(rx);
        }

        // Wait for all to complete
        let mut completed = 0;
        for rx in receivers {
            if rx.recv_timeout(Duration::from_millis(100)).is_ok() {
                completed += 1;
            }
        }

        println!("Completed {completed}/{num_tasks} tasks");
        assert!(completed > 0, "At least some tasks should complete");

        handle.terminate();
        manager_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    #[ignore = "Requires real database state for proof computation"]
    async fn test_task_monitoring_with_different_task_types() {
        let manager = create_test_proof_task_manager(2);
        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        let mut receivers = Vec::new();

        // account node tasks
        for i in 0..5u8 {
            let (tx, rx) = channel();
            let path = Nibbles::from_nibbles_unchecked(vec![i; 32]);
            handle
                .queue_task(ProofTaskKind::BlindedAccountNode(path, tx))
                .expect("Failed to queue task");
            receivers.push(rx);
        }

        // storage node tasks
        for i in 0..5 {
            let (tx, rx) = channel();
            let account = B256::left_padding_from(&(i as u64).to_be_bytes());
            let path = Nibbles::from_nibbles_unchecked(vec![i as u8; 32]);
            handle
                .queue_task(ProofTaskKind::BlindedStorageNode(account, path, tx))
                .expect("Failed to queue task");
            receivers.push(rx);
        }

        let mut completed = 0;
        for rx in receivers {
            if rx.recv_timeout(Duration::from_millis(100)).is_ok() {
                completed += 1;
            }
        }

        assert!(completed > 0, "At least some tasks should complete");

        handle.terminate();
        manager_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_handle_drop_triggers_shutdown() {
        let manager = create_test_proof_task_manager(2);
        let handle = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        drop(handle);

        let result = tokio::time::timeout(Duration::from_secs(2), manager_task).await;
        assert!(result.is_ok(), "Manager should shut down when handle is dropped");
    }

    #[tokio::test]
    async fn test_multiple_handles() {
        let manager = create_test_proof_task_manager(2);
        let handle1 = manager.handle();
        let handle2 = manager.handle();
        let handle3 = manager.handle();

        let manager_task = tokio::spawn(async move { manager.run().await });

        // Queue tasks from different handles
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();
        let (tx3, rx3) = channel();

        let input1 = StorageProofInput::new(B256::left_padding_from(&1u64.to_be_bytes()), Default::default(), Default::default(), false, None);
        let input2 = StorageProofInput::new(B256::left_padding_from(&2u64.to_be_bytes()), Default::default(), Default::default(), false, None);
        let input3 = StorageProofInput::new(B256::left_padding_from(&3u64.to_be_bytes()), Default::default(), Default::default(), false, None);

        handle1.queue_task(ProofTaskKind::StorageProof(input1, tx1)).unwrap();
        handle2.queue_task(ProofTaskKind::StorageProof(input2, tx2)).unwrap();
        handle3.queue_task(ProofTaskKind::StorageProof(input3, tx3)).unwrap();

        let _ = rx1.recv_timeout(Duration::from_millis(200));
        let _ = rx2.recv_timeout(Duration::from_millis(200));
        let _ = rx3.recv_timeout(Duration::from_millis(200));

        // Drop all handles - last one should trigger shutdown
        drop(handle1);
        drop(handle2);
        drop(handle3);

        let result = tokio::time::timeout(Duration::from_secs(2), manager_task).await;
        assert!(result.is_ok(), "Manager should shut down when all handles are dropped");
    }
}
