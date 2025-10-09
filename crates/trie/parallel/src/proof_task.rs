//! A Task that manages sending proof requests to a number of tasks that have longer-running
//! database transactions.
//!
//! The [`ProofTaskManager`] ensures that there are a max number of currently executing proof tasks,
//! and is responsible for managing the fixed number of database transactions created at the start
//! of the task.
//!
//! Individual [`ProofTaskTx`] instances manage a dedicated [`InMemoryTrieCursorFactory`] and
//! [`HashedPostStateCursorFactory`], which are each backed by a database transaction.

use crate::{
    root::ParallelStateRootError,
    stats::{ParallelTrieStats, ParallelTrieTracker},
    StorageRootTargets,
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use reth_db_api::transaction::DbTx;
use reth_execution_errors::SparseTrieError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderError, ProviderResult,
};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{TriePrefixSets, TriePrefixSetsMut},
    proof::{ProofTrieNodeProviderFactory, StorageProof},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    walker::TrieWalker,
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostStateSorted,
    MultiProofTargets, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
    proof::{DecodedProofNodes, ProofRetainer},
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_sparse::provider::{RevealedNode, TrieNodeProvider, TrieNodeProviderFactory};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
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
type AccountMultiproofResult =
    Result<(DecodedMultiProof, ParallelTrieStats), ParallelStateRootError>;

/// Worker type identifier
#[derive(Debug)]
enum WorkerType {
    /// Storage proof worker
    Storage,
    /// Account multiproof worker
    Account,
}

impl std::fmt::Display for WorkerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage => write!(f, "Storage"),
            Self::Account => write!(f, "Account"),
        }
    }
}

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
/// This manager operates two distinct worker pools for parallel trie operations:
///
/// **Worker Pools**:
///    - Pre-spawned workers with dedicated long-lived transactions
///    - **Storage pool**: Handles `StorageProof` and `BlindedStorageNode` requests
///    - **Account pool**: Handles `AccountMultiproof` and `BlindedAccountNode` requests, delegates
///      storage proof computation to storage pool
///    - Tasks queued via crossbeam unbounded channels
///    - Workers continuously process without transaction overhead
///    - Returns error if worker pool is unavailable (all workers panicked)
///
/// # Public Interface
///
/// The public interface through `ProofTaskManagerHandle` allows external callers to:
/// - Submit tasks via `queue_task(ProofTaskKind)`
/// - Use standard `std::mpsc` message passing
/// - Receive consistent return types and error handling
#[derive(Debug)]
pub struct ProofTaskManager {
    /// Sender for storage worker jobs to worker pool.
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,

    /// Number of storage workers successfully spawned.
    ///
    /// May be less than requested if concurrency limits reduce the worker budget.
    storage_worker_count: usize,

    /// Sender for account worker jobs to worker pool.
    account_work_tx: CrossbeamSender<AccountWorkerJob>,

    /// Number of account workers successfully spawned.
    account_worker_count: usize,

    /// Receives proof task requests from [`ProofTaskManagerHandle`].
    proof_task_rx: CrossbeamReceiver<ProofTaskMessage>,

    /// Sender for creating handles that can queue tasks.
    proof_task_tx: CrossbeamSender<ProofTaskMessage>,

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

// TODO: Refactor this with storage_worker_loop. ProofTaskManager should be removed in the following
// pr and `MultiproofManager` should be used instead to dispatch jobs directly.
/// Worker loop for account trie operations.
///
/// # Lifecycle
///
/// Each worker:
/// 1. Receives `AccountWorkerJob` from crossbeam unbounded channel
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
fn account_worker_loop<Tx>(
    proof_tx: ProofTaskTx<Tx>,
    work_rx: CrossbeamReceiver<AccountWorkerJob>,
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    worker_id: usize,
) where
    Tx: DbTx,
{
    tracing::debug!(
        target: "trie::proof_task",
        worker_id,
        "Account worker started"
    );

    // Create factories once at worker startup to avoid recreation overhead.
    let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();

    // Create blinded provider factory once for all blinded node requests
    let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
        trie_cursor_factory.clone(),
        hashed_cursor_factory.clone(),
        proof_tx.task_ctx.prefix_sets.clone(),
    );

    let mut account_proofs_processed = 0u64;
    let mut account_nodes_processed = 0u64;

    while let Ok(job) = work_rx.recv() {
        match job {
            AccountWorkerJob::AccountMultiproof { mut input, result_sender } => {
                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    targets = input.targets.len(),
                    "Processing account multiproof"
                );

                let proof_start = Instant::now();
                let mut tracker = ParallelTrieTracker::default();

                let mut storage_prefix_sets =
                    std::mem::take(&mut input.prefix_sets.storage_prefix_sets);

                let storage_root_targets_len = StorageRootTargets::count(
                    &input.prefix_sets.account_prefix_set,
                    &storage_prefix_sets,
                );
                tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

                let storage_proof_receivers = match queue_storage_proofs(
                    &storage_work_tx,
                    &input.targets,
                    &mut storage_prefix_sets,
                    input.collect_branch_node_masks,
                    input.multi_added_removed_keys.as_ref(),
                ) {
                    Ok(receivers) => receivers,
                    Err(error) => {
                        let _ = result_sender.send(Err(error));
                        continue;
                    }
                };

                // Use the missed leaves cache passed from the multiproof manager
                let missed_leaves_storage_roots = &input.missed_leaves_storage_roots;

                let account_prefix_set = std::mem::take(&mut input.prefix_sets.account_prefix_set);

                let result = build_account_multiproof_with_storage_roots(
                    trie_cursor_factory.clone(),
                    hashed_cursor_factory.clone(),
                    &input.targets,
                    account_prefix_set,
                    input.collect_branch_node_masks,
                    input.multi_added_removed_keys.as_ref(),
                    storage_proof_receivers,
                    missed_leaves_storage_roots,
                    &mut tracker,
                );

                let proof_elapsed = proof_start.elapsed();
                let stats = tracker.finish();
                let result = result.map(|proof| (proof, stats));
                account_proofs_processed += 1;

                if result_sender.send(result).is_err() {
                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        account_proofs_processed,
                        "Account multiproof receiver dropped, discarding result"
                    );
                }

                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    proof_time_us = proof_elapsed.as_micros(),
                    total_processed = account_proofs_processed,
                    "Account multiproof completed"
                );
            }

            AccountWorkerJob::BlindedAccountNode { path, result_sender } => {
                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    ?path,
                    "Processing blinded account node"
                );

                let start = Instant::now();
                let result = blinded_provider_factory.account_node_provider().trie_node(&path);
                let elapsed = start.elapsed();

                account_nodes_processed += 1;

                if result_sender.send(result).is_err() {
                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        ?path,
                        account_nodes_processed,
                        "Blinded account node receiver dropped, discarding result"
                    );
                }

                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    ?path,
                    node_time_us = elapsed.as_micros(),
                    total_processed = account_nodes_processed,
                    "Blinded account node completed"
                );
            }
        }
    }

    tracing::debug!(
        target: "trie::proof_task",
        worker_id,
        account_proofs_processed,
        account_nodes_processed,
        "Account worker shutting down"
    );
}

/// Builds an account multiproof by consuming storage proof receivers lazily during trie walk.
///
/// This is a helper function used by account workers to build the account subtree proof
/// while storage proofs are still being computed. Receivers are consumed only when needed,
/// enabling interleaved parallelism between account trie traversal and storage proof computation.
///
/// Returns a `DecodedMultiProof` containing the account subtree and storage proofs.
#[allow(clippy::too_many_arguments)]
fn build_account_multiproof_with_storage_roots<C, H>(
    trie_cursor_factory: C,
    hashed_cursor_factory: H,
    targets: &MultiProofTargets,
    prefix_set: PrefixSet,
    collect_branch_node_masks: bool,
    multi_added_removed_keys: Option<&Arc<MultiAddedRemovedKeys>>,
    mut storage_proof_receivers: B256Map<Receiver<StorageProofResult>>,
    missed_leaves_storage_roots: &DashMap<B256, B256>,
    tracker: &mut ParallelTrieTracker,
) -> Result<DecodedMultiProof, ParallelStateRootError>
where
    C: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    let accounts_added_removed_keys =
        multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

    // Create the walker.
    let walker = TrieWalker::<_>::state_trie(
        trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
        prefix_set,
    )
    .with_added_removed_keys(accounts_added_removed_keys)
    .with_deletions_retained(true);

    // Create a hash builder to rebuild the root node since it is not available in the database.
    let retainer = targets
        .keys()
        .map(Nibbles::unpack)
        .collect::<ProofRetainer>()
        .with_added_removed_keys(accounts_added_removed_keys);
    let mut hash_builder = HashBuilder::default()
        .with_proof_retainer(retainer)
        .with_updates(collect_branch_node_masks);

    // Initialize storage multiproofs map with pre-allocated capacity.
    // Proofs will be inserted as they're consumed from receivers during trie walk.
    let mut collected_decoded_storages: B256Map<DecodedStorageMultiProof> =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());
    let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    let mut account_node_iter = TrieNodeIter::state_trie(
        walker,
        hashed_cursor_factory.hashed_account_cursor().map_err(ProviderError::Database)?,
    );

    while let Some(account_node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
        match account_node {
            TrieElement::Branch(node) => {
                hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
            }
            TrieElement::Leaf(hashed_address, account) => {
                let root = match storage_proof_receivers.remove(&hashed_address) {
                    Some(receiver) => {
                        // Block on this specific storage proof receiver - enables interleaved
                        // parallelism
                        let proof = receiver.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(
                                reth_execution_errors::StorageRootError::Database(
                                    DatabaseError::Other(format!(
                                        "Storage proof channel closed for {hashed_address}"
                                    )),
                                ),
                            )
                        })??;
                        let root = proof.root;
                        collected_decoded_storages.insert(hashed_address, proof);
                        root
                    }
                    // Since we do not store all intermediate nodes in the database, there might
                    // be a possibility of re-adding a non-modified leaf to the hash builder.
                    None => {
                        tracker.inc_missed_leaves();

                        match missed_leaves_storage_roots.entry(hashed_address) {
                            dashmap::Entry::Occupied(occ) => *occ.get(),
                            dashmap::Entry::Vacant(vac) => {
                                let root = StorageProof::new_hashed(
                                    trie_cursor_factory.clone(),
                                    hashed_cursor_factory.clone(),
                                    hashed_address,
                                )
                                .with_prefix_set_mut(Default::default())
                                .storage_multiproof(
                                    targets.get(&hashed_address).cloned().unwrap_or_default(),
                                )
                                .map_err(|e| {
                                    ParallelStateRootError::StorageRoot(
                                        reth_execution_errors::StorageRootError::Database(
                                            DatabaseError::Other(e.to_string()),
                                        ),
                                    )
                                })?
                                .root;

                                vac.insert(root);
                                root
                            }
                        }
                    }
                };

                // Encode account
                account_rlp.clear();
                let account = account.into_trie_account(root);
                account.encode(&mut account_rlp as &mut dyn BufMut);

                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
            }
        }
    }

    // Consume remaining storage proof receivers for accounts not encountered during trie walk.
    for (hashed_address, receiver) in storage_proof_receivers {
        if let Ok(Ok(proof)) = receiver.recv() {
            collected_decoded_storages.insert(hashed_address, proof);
        }
    }

    let _ = hash_builder.root();

    let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
    let decoded_account_subtree = DecodedProofNodes::try_from(account_subtree_raw_nodes)?;

    let (branch_node_hash_masks, branch_node_tree_masks) = if collect_branch_node_masks {
        let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
        (
            updated_branch_nodes.iter().map(|(path, node)| (*path, node.hash_mask)).collect(),
            updated_branch_nodes.into_iter().map(|(path, node)| (path, node.tree_mask)).collect(),
        )
    } else {
        (Default::default(), Default::default())
    };

    Ok(DecodedMultiProof {
        account_subtree: decoded_account_subtree,
        branch_node_hash_masks,
        branch_node_tree_masks,
        storages: collected_decoded_storages,
    })
}

/// Queues storage proofs for all accounts in the targets and returns receivers.
///
/// This function queues all storage proof tasks to the worker pool but returns immediately
/// with receivers, allowing the account trie walk to proceed in parallel with storage proof
/// computation. This enables interleaved parallelism for better performance.
///
/// Propagates errors up if queuing fails. Receivers must be consumed by the caller.
fn queue_storage_proofs(
    storage_work_tx: &CrossbeamSender<StorageWorkerJob>,
    targets: &MultiProofTargets,
    storage_prefix_sets: &mut B256Map<PrefixSet>,
    with_branch_node_masks: bool,
    multi_added_removed_keys: Option<&Arc<MultiAddedRemovedKeys>>,
) -> Result<B256Map<Receiver<StorageProofResult>>, ParallelStateRootError> {
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());

    // Queue all storage proofs to worker pool
    for (hashed_address, target_slots) in targets.iter() {
        let prefix_set = storage_prefix_sets.remove(hashed_address).unwrap_or_default();

        // Always queue a storage proof so we obtain the storage root even when no slots are
        // requested.
        let input = StorageProofInput::new(
            *hashed_address,
            prefix_set,
            target_slots.clone(),
            with_branch_node_masks,
            multi_added_removed_keys.cloned(),
        );

        let (sender, receiver) = channel();

        // If queuing fails, propagate error up (no fallback)
        storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, result_sender: sender })
            .map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "Failed to queue storage proof for {}: storage worker pool unavailable",
                    hashed_address
                ))
            })?;

        storage_proof_receivers.insert(*hashed_address, receiver);
    }

    Ok(storage_proof_receivers)
}

impl ProofTaskManager {
    /// Creates a new [`ProofTaskManager`] with pre-spawned storage and account proof workers.
    ///
    /// The `storage_worker_count` determines how many storage workers to spawn, and
    /// `account_worker_count` determines how many account workers to spawn.
    /// Returns an error if the underlying provider fails to create the transactions required for
    /// spawning workers.
    pub fn new<Factory>(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        storage_worker_count: usize,
        account_worker_count: usize,
    ) -> ProviderResult<Self>
    where
        Factory: DatabaseProviderFactory<Provider: BlockReader>,
    {
        // Use unbounded channel for the router to prevent account workers from blocking
        // when queuing storage proofs. Account workers queue many storage proofs through
        // this channel, and blocking on a bounded channel wastes parallel worker capacity.
        let (proof_task_tx, proof_task_rx) = unbounded();

        // Use unbounded channel to ensure all storage operations are queued to workers.
        // This maintains transaction reuse benefits and avoids fallback to on-demand execution.
        let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();
        let (account_work_tx, account_work_rx) = unbounded::<AccountWorkerJob>();

        tracing::info!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            "Initializing storage and account worker pools with unbounded queues"
        );

        // Spawn storage workers
        let spawned_storage_workers = Self::spawn_worker_pool(
            &executor,
            &view,
            &task_ctx,
            storage_worker_count,
            storage_work_rx,
            WorkerType::Storage,
            storage_worker_loop,
        )?;

        // Spawn account workers with direct access to the storage worker queue.
        let spawned_account_workers = Self::spawn_worker_pool(
            &executor,
            &view,
            &task_ctx,
            account_worker_count,
            account_work_rx,
            WorkerType::Account,
            {
                let storage_work_tx = storage_work_tx.clone();
                move |proof_tx, work_rx, worker_id| {
                    account_worker_loop(proof_tx, work_rx, storage_work_tx.clone(), worker_id)
                }
            },
        )?;

        Ok(Self {
            storage_work_tx,
            storage_worker_count: spawned_storage_workers,
            account_work_tx,
            account_worker_count: spawned_account_workers,
            proof_task_rx,
            proof_task_tx,
            active_handles: Arc::new(AtomicUsize::new(0)),

            #[cfg(feature = "metrics")]
            metrics: ProofTaskMetrics::default(),
        })
    }

    /// Returns a handle for sending new proof tasks to the [`ProofTaskManager`].
    pub fn handle(&self) -> ProofTaskManagerHandle {
        ProofTaskManagerHandle::new(self.proof_task_tx.clone(), self.active_handles.clone())
    }

    /// Spawns a pool of workers with dedicated database transactions.
    ///
    /// # Type Parameters
    /// - `Job`: The job type the workers will process
    /// - `F`: The worker loop function type
    ///
    /// # Parameters
    /// - `worker_count`: Number of workers to spawn
    /// - `work_rx`: Receiver for the worker job channel
    /// - `worker_type`: Type of worker for logging
    /// - `worker_fn`: The worker loop function to execute
    ///
    /// Returns
    /// The number of workers successfully spawned
    fn spawn_worker_pool<Factory, Job, F>(
        executor: &Handle,
        view: &ConsistentDbView<Factory>,
        task_ctx: &ProofTaskCtx,
        worker_count: usize,
        work_rx: CrossbeamReceiver<Job>,
        worker_type: WorkerType,
        worker_fn: F,
    ) -> ProviderResult<usize>
    where
        Factory: DatabaseProviderFactory<Provider: BlockReader>,
        Job: Send + 'static,
        F: Fn(ProofTaskTx<FactoryTx<Factory>>, CrossbeamReceiver<Job>, usize)
            + Send
            + Clone
            + 'static,
    {
        let mut spawned_workers = 0;
        // spawns workers that will execute worker_fn
        for worker_id in 0..worker_count {
            let provider_ro = view.provider_ro()?;
            let tx = provider_ro.into_tx();
            let proof_task_tx = ProofTaskTx::new(tx, task_ctx.clone(), worker_id);
            let work_rx_clone = work_rx.clone();
            let worker_fn_clone = worker_fn.clone();

            executor
                .spawn_blocking(move || worker_fn_clone(proof_task_tx, work_rx_clone, worker_id));

            spawned_workers += 1;

            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                spawned_workers,
                worker_type = %worker_type,
                "{} worker spawned successfully", worker_type
            );
        }

        Ok(spawned_workers)
    }

    /// Loops, managing the proof tasks, routing them to the appropriate worker pools.
    ///
    /// # Task Routing
    ///
    /// - **Storage Trie Operations** (`StorageProof` and `BlindedStorageNode`): Routed to
    ///   pre-spawned storage worker pool via unbounded channel. Returns error if workers are
    ///   disconnected (e.g., all workers panicked).
    /// - **Account Trie Operations** (`AccountMultiproof` and `BlindedAccountNode`): Routed to
    ///   pre-spawned account worker pool via unbounded channel. Returns error if workers are
    ///   disconnected.
    ///
    /// # Shutdown
    ///
    /// On termination, `storage_work_tx` and `account_work_tx` are dropped, closing the channels
    /// and signaling all workers to shut down gracefully.
    pub fn run(mut self) -> ProviderResult<()> {
        loop {
            match self.proof_task_rx.recv() {
                Ok(message) => {
                    match message {
                        ProofTaskMessage::QueueTask(task) => match task {
                            ProofTaskKind::StorageProof(input, sender) => {
                                self.storage_work_tx
                                    .send(StorageWorkerJob::StorageProof {
                                        input,
                                        result_sender: sender,
                                    })
                                    .expect("storage workers are running until Terminate");

                                tracing::trace!(
                                    target: "trie::proof_task",
                                    "Storage proof dispatched to worker pool"
                                );
                            }

                            ProofTaskKind::BlindedStorageNode(account, path, sender) => {
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.storage_nodes += 1;
                                }

                                self.storage_work_tx
                                    .send(StorageWorkerJob::BlindedStorageNode {
                                        account,
                                        path,
                                        result_sender: sender,
                                    })
                                    .expect("storage workers are running until Terminate");

                                tracing::trace!(
                                    target: "trie::proof_task",
                                    ?account,
                                    ?path,
                                    "Blinded storage node dispatched to worker pool"
                                );
                            }

                            ProofTaskKind::BlindedAccountNode(path, sender) => {
                                #[cfg(feature = "metrics")]
                                {
                                    self.metrics.account_nodes += 1;
                                }

                                self.account_work_tx
                                    .send(AccountWorkerJob::BlindedAccountNode {
                                        path,
                                        result_sender: sender,
                                    })
                                    .expect("account workers are running until Terminate");

                                tracing::trace!(
                                    target: "trie::proof_task",
                                    ?path,
                                    "Blinded account node dispatched to worker pool"
                                );
                            }

                            ProofTaskKind::AccountMultiproof(input, sender) => {
                                self.account_work_tx
                                    .send(AccountWorkerJob::AccountMultiproof {
                                        input,
                                        result_sender: sender,
                                    })
                                    .expect("account workers are running until Terminate");

                                tracing::trace!(
                                    target: "trie::proof_task",
                                    "Account multiproof dispatched to worker pool"
                                );
                            }
                        },
                        ProofTaskMessage::Terminate => {
                            // Drop worker channels to signal workers to shut down
                            drop(self.storage_work_tx);
                            drop(self.account_work_tx);

                            tracing::debug!(
                                target: "trie::proof_task",
                                storage_worker_count = self.storage_worker_count,
                                account_worker_count = self.account_worker_count,
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

    /// Compute storage proof without consuming self.
    ///
    /// Borrows self immutably to allow transaction reuse across multiple calls.
    /// Used by storage workers in the worker pool to avoid transaction creation
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

/// Input parameters for account multiproof computation.
#[derive(Debug, Clone)]
pub struct AccountMultiproofInput {
    /// The targets for which to compute the multiproof.
    pub targets: MultiProofTargets,
    /// The prefix sets for the proof calculation.
    pub prefix_sets: TriePrefixSets,
    /// Whether or not to collect branch node masks.
    pub collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    pub missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
}

/// Internal message for account workers.
///
/// This is NOT exposed publicly. External callers use `ProofTaskKind::AccountMultiproof` or
/// `ProofTaskKind::BlindedAccountNode` which are routed through the manager's `std::mpsc` channel.
#[derive(Debug)]
enum AccountWorkerJob {
    /// Account multiproof computation request
    AccountMultiproof {
        /// Account multiproof input parameters
        input: AccountMultiproofInput,
        /// Channel to send result back to original caller
        result_sender: Sender<AccountMultiproofResult>,
    },
    /// Blinded account node retrieval request
    BlindedAccountNode {
        /// Path to the account node
        path: Nibbles,
        /// Channel to send result back to original caller
        result_sender: Sender<TrieNodeProviderResult>,
    },
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
    /// An account multiproof request.
    AccountMultiproof(AccountMultiproofInput, Sender<AccountMultiproofResult>),
}

/// A handle that wraps a single proof task sender that sends a terminate message on `Drop` if the
/// number of active handles went to zero.
#[derive(Debug)]
pub struct ProofTaskManagerHandle {
    /// The sender for the proof task manager.
    sender: CrossbeamSender<ProofTaskMessage>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
}

impl ProofTaskManagerHandle {
    /// Creates a new [`ProofTaskManagerHandle`] with the given sender.
    pub fn new(
        sender: CrossbeamSender<ProofTaskMessage>,
        active_handles: Arc<AtomicUsize>,
    ) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self { sender, active_handles }
    }

    /// Queues a task to the proof task manager.
    pub fn queue_task(
        &self,
        task: ProofTaskKind,
    ) -> Result<(), crossbeam_channel::SendError<ProofTaskMessage>> {
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
        sender: CrossbeamSender<ProofTaskMessage>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Sender to the proof task.
        sender: CrossbeamSender<ProofTaskMessage>,
    },
}

impl TrieNodeProvider for ProofTaskTrieNodeProvider {
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

    /// Ensures `max_concurrency` is independent of storage and account workers.
    #[test]
    fn proof_task_manager_independent_pools() {
        let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = tokio::runtime::Handle::current();
            let factory = create_test_provider_factory();
            let view = ConsistentDbView::new(factory, None);
            let ctx = test_ctx();

            let manager = ProofTaskManager::new(handle.clone(), view, ctx, 5, 3).unwrap();
            // With storage_worker_count=5, we get exactly 5 storage workers
            assert_eq!(manager.storage_worker_count, 5);
            // With account_worker_count=3, we get exactly 3 account workers
            assert_eq!(manager.account_worker_count, 3);

            drop(manager);
            task::yield_now().await;
        });
    }
}
