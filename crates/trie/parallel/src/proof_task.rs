//! A Task that manages sending proof requests to a number of tasks that have longer-running
//! database transactions.
//!
//! The [`spawn_proof_workers`] function spawns worker threads with pre-warmed database
//! transactions. Proof jobs are dispatched directly to workers via [`ProofTaskManagerHandle`].
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
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
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
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::{debug, error, warn};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskMetrics;

pub(crate) type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// Error when storage manager is closed.
#[inline]
fn storage_manager_closed_error() -> ParallelStateRootError {
    ParallelStateRootError::Other("storage manager closed".into())
}

/// Executes an account multiproof task in a worker thread.
///
/// This function coordinates with the storage manager to compute storage proofs for all
/// accounts in the multiproof, then assembles the final account multiproof by fetching
/// storage proofs on-demand during account trie traversal.
///
/// Returns the multiproof result. The caller is responsible for sending via `result_sender`.
fn execute_account_multiproof_worker<Tx: DbTx>(
    targets: MultiProofTargets,
    prefix_sets: TriePrefixSets,
    collect_branch_node_masks: bool,
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    storage_proof_handle: ProofTaskManagerHandle<Tx>,
    proof_tx: &ProofTaskTx<Tx>,
) -> Result<DecodedMultiProof, ParallelStateRootError> {
    // Build account multiproof, fetching storage proofs on-demand during trie traversal
    let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();

    // Extend prefix sets with targets to ensure all intermediate nodes are revealed.
    // We must preserve the `all` flag from existing prefix sets (e.g., for wiped storage).
    // Clone frozen prefix sets, convert to mutable, extend with targets, then freeze again.
    let mut extended_prefix_sets = reth_trie::prefix_set::TriePrefixSetsMut {
        account_prefix_set: {
            if prefix_sets.account_prefix_set.all() {
                // If all accounts changed, keep the all flag
                reth_trie::prefix_set::PrefixSetMut::all()
            } else {
                // Otherwise, start with existing keys
                reth_trie::prefix_set::PrefixSetMut::from(
                    prefix_sets.account_prefix_set.iter().copied(),
                )
            }
        },
        storage_prefix_sets: {
            let mut storage_sets = B256Map::default();
            for (address, prefix_set) in &prefix_sets.storage_prefix_sets {
                let mutable_set = if prefix_set.all() {
                    reth_trie::prefix_set::PrefixSetMut::all()
                } else {
                    reth_trie::prefix_set::PrefixSetMut::from(prefix_set.iter().copied())
                };
                storage_sets.insert(*address, mutable_set);
            }
            storage_sets
        },
        destroyed_accounts: prefix_sets.destroyed_accounts,
    };

    // Now extend with targets. Using .extend() properly preserves the `all` flag.
    extended_prefix_sets.extend(reth_trie::prefix_set::TriePrefixSetsMut {
        account_prefix_set: reth_trie::prefix_set::PrefixSetMut::from(
            targets.keys().copied().map(Nibbles::unpack),
        ),
        storage_prefix_sets: targets
            .iter()
            .filter(|&(_address, slots)| !slots.is_empty())
            .map(|(address, slots)| {
                (
                    *address,
                    reth_trie::prefix_set::PrefixSetMut::from(slots.iter().map(Nibbles::unpack)),
                )
            })
            .collect(),
        destroyed_accounts: Default::default(),
    });

    let extended_prefix_sets = extended_prefix_sets.freeze();

    debug!(
        target: "trie::proof_task",
        targets_count = targets.len(),
        account_prefix_set_len = extended_prefix_sets.account_prefix_set.len(),
        storage_prefix_sets_len = extended_prefix_sets.storage_prefix_sets.len(),
        "[WORKER] About to queue storage proofs for all accounts in extended prefix set"
    );

    // Queue storage proof requests for ALL accounts we'll visit during trie traversal.
    // The walker visits all accounts in extended_prefix_sets.account_prefix_set, so we need
    // to queue storage proofs for all of them, even if they have no storage changes or empty slots.
    let mut storage_receivers: B256Map<
        CrossbeamReceiver<Result<DecodedStorageMultiProof, ParallelStateRootError>>,
    > = B256Map::default();

    for account_nibbles in &extended_prefix_sets.account_prefix_set {
        // Convert nibbles back to B256 address
        let address = B256::from_slice(&account_nibbles.pack());

        let (sender, receiver) = crossbeam_channel::unbounded();
        let prefix_set =
            extended_prefix_sets.storage_prefix_sets.get(&address).cloned().unwrap_or_default();
        let target_slots = targets.get(&address).cloned().unwrap_or_default();

        let storage_input = StorageProofInput::new(
            address,
            prefix_set,
            Arc::new(target_slots),
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
            return Err(storage_manager_closed_error());
        }

        storage_receivers.insert(address, receiver);
    }

    debug!(
        target: "trie::proof_task",
        receivers_queued = storage_receivers.len(),
        "[WORKER] Queued all storage proof receivers, calling build_account_multiproof_with_storage"
    );

    let result = crate::proof::build_account_multiproof_with_storage(
        trie_cursor_factory,
        hashed_cursor_factory,
        targets,
        extended_prefix_sets,
        storage_receivers, // ← Pass receivers directly for on-demand fetching
        collect_branch_node_masks,
        multi_added_removed_keys,
    )?;

    // Return multiproof without stats
    Ok(result.0)
}

/// Proof job dispatched to the storage worker pool.
///
/// Storage workers handle storage proofs and blinded node requests since both
/// operate on trie data with similar transaction requirements.
#[derive(Debug)]
pub enum ProofJob<Tx> {
    /// Storage proof request
    StorageProof {
        /// Input for the storage proof
        input: StorageProofInput,
        /// Channel to send result
        result_tx: CrossbeamSender<StorageProofResult>,
        /// Timestamp when enqueued (for metrics)
        #[cfg(feature = "metrics")]
        enqueued_at: Instant,
        /// Phantom data to maintain type parameter
        #[doc(hidden)]
        _phantom: std::marker::PhantomData<Tx>,
    },
    /// Blinded account node request (reuses worker's pre-warmed tx)
    BlindedAccountNode {
        /// Node path to retrieve
        path: Nibbles,
        /// Result channel
        result_tx: CrossbeamSender<TrieNodeProviderResult>,
        /// Phantom data to maintain type parameter
        #[doc(hidden)]
        _phantom: std::marker::PhantomData<Tx>,
    },
    /// Blinded storage node request (reuses worker's pre-warmed tx)
    BlindedStorageNode {
        /// Account hash
        account: B256,
        /// Node path to retrieve
        path: Nibbles,
        /// Result channel
        result_tx: CrossbeamSender<TrieNodeProviderResult>,
        /// Phantom data to maintain type parameter
        #[doc(hidden)]
        _phantom: std::marker::PhantomData<Tx>,
    },
}

/// Account multiproof job dispatched to the account worker pool.
#[derive(Debug)]
pub struct AccountProofJob<Tx> {
    /// Input for the account multiproof calculation
    pub(crate) input: AccountMultiproofInput<Tx>,
    /// Timestamp when the job was enqueued (for metrics)
    #[cfg(feature = "metrics")]
    pub(crate) enqueued_at: Instant,
}

/// Spawns proof workers for storage and account multiproofs.
///
/// This function spawns worker threads that maintain long-lived database transactions
/// for efficient proof computation. Workers reuse these transactions across multiple proofs
/// following the prewarm.rs pattern.
///
/// # Arguments
///
/// * `executor` - Tokio runtime handle for spawning blocking tasks
/// * `view` - Consistent database view for creating read-only transactions
/// * `task_ctx` - Shared context (trie updates, state, prefix sets) for all workers
/// * `storage_worker_count` - Number of storage proof workers (clamped to minimum 1)
/// * `account_worker_count` - Number of account multiproof workers (clamped to minimum 1)
/// * `_max_concurrency` - Reserved for future use
///
/// # Returns
///
/// A handle for dispatching proof jobs to the worker pools.
///
/// # Errors
///
/// Returns an error if the consistent view provider fails to create read-only transactions.
pub fn spawn_proof_workers<Factory>(
    executor: Handle,
    view: ConsistentDbView<Factory>,
    task_ctx: ProofTaskCtx,
    storage_worker_count: usize,
    account_worker_count: usize,
    _max_concurrency: usize,
) -> ProviderResult<ProofTaskManagerHandle<FactoryTx<Factory>>>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static,
{
    // Clamp worker counts to at least 1
    let storage_worker_count = storage_worker_count.max(1);
    let account_worker_count = account_worker_count.max(1);

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
                    let job: ProofJob<_> = match storage_work_rx.recv() {
                        Ok(item) => item,
                        Err(_) => break, // Channel closed, shutdown
                    };

                    match job {
                        ProofJob::StorageProof { input, result_tx, #[cfg(feature = "metrics")] enqueued_at, .. } => {
                            #[cfg(feature = "metrics")]
                            {
                                let depth = storage_queue_depth_clone
                                    .fetch_sub(1, Ordering::SeqCst)
                                    .saturating_sub(1);
                                metrics_clone.record_storage_queue_depth(depth);
                                metrics_clone.record_storage_wait_time(enqueued_at.elapsed());
                            }

                            // Wrap proof computation in panic recovery to prevent zombie workers
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                proof_tx.storage_proof_internal(&input)
                            }));

                            let final_result = match result {
                                Ok(Ok(proof)) => Ok(proof),
                                Ok(Err(e)) => Err(e),
                                Err(_panic_info) => {
                                    error!(
                                        target: "trie::proof_pool",
                                        worker_id,
                                        hashed_address = ?input.hashed_address,
                                        "Storage proof task panicked, converting to error"
                                    );
                                    Err(ParallelStateRootError::Other(format!(
                                        "storage proof task panicked for {}",
                                        input.hashed_address
                                    )))
                                }
                            };

                            let _ = result_tx.send(final_result);
                        }

                        ProofJob::BlindedAccountNode { path, result_tx, .. } => {
                            // Use worker's pre-warmed proof_tx (no fresh transaction needed)
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();
                                let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
                                    trie_cursor_factory,
                                    hashed_cursor_factory,
                                    proof_tx.task_ctx.prefix_sets.clone(),
                                );
                                blinded_provider_factory.account_node_provider().trie_node(&path)
                            }));

                            let final_result = match result {
                                Ok(r) => r,
                                Err(_panic_info) => {
                                    error!(target: "trie::proof_pool", worker_id, ?path,
                                        "Blinded account node task panicked");
                                    Err(SparseTrieErrorKind::Other(Box::new(
                                        std::io::Error::other(
                                            format!("blinded account node task panicked for path {:?}", path))
                                    )).into())
                                }
                            };

                            let _ = result_tx.send(final_result);
                        }

                        ProofJob::BlindedStorageNode { account, path, result_tx, .. } => {
                            // Use worker's pre-warmed proof_tx (no fresh transaction needed)
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let (trie_cursor_factory, hashed_cursor_factory) = proof_tx.create_factories();
                                let blinded_provider_factory = ProofTrieNodeProviderFactory::new(
                                    trie_cursor_factory,
                                    hashed_cursor_factory,
                                    proof_tx.task_ctx.prefix_sets.clone(),
                                );
                                blinded_provider_factory.storage_node_provider(account).trie_node(&path)
                            }));

                            let final_result = match result {
                                Ok(r) => r,
                                Err(_panic_info) => {
                                    error!(target: "trie::proof_pool", worker_id, ?account, ?path,
                                        "Blinded storage node task panicked");
                                    Err(SparseTrieErrorKind::Other(Box::new(
                                        std::io::Error::other(
                                            format!("blinded storage node task panicked for account {:?} path {:?}",
                                                account, path))
                                    )).into())
                                }
                            };

                            let _ = result_tx.send(final_result);
                        }
                    }
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
                    let depth =
                        account_queue_depth_clone.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                    metrics_clone.record_account_queue_depth(depth);
                    metrics_clone.record_account_wait_time(job.enqueued_at.elapsed());
                }

                // Destructure input to extract result_sender and pass fields to worker
                let AccountMultiproofInput {
                    targets,
                    prefix_sets,
                    collect_branch_node_masks,
                    multi_added_removed_keys,
                    storage_proof_handle,
                    result_sender,
                } = job.input;

                // Wrap account multiproof execution in panic recovery
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_account_multiproof_worker(
                        targets,
                        prefix_sets,
                        collect_branch_node_masks,
                        multi_added_removed_keys,
                        storage_proof_handle,
                        &proof_tx,
                    )
                }));

                // Convert panic to error and send result
                let final_result = match result {
                    Ok(Ok(multiproof)) => Ok(multiproof),
                    Ok(Err(e)) => Err(e),
                    Err(_panic_info) => {
                        error!(
                            target: "trie::proof_pool",
                            worker_id,
                            "Account multiproof task panicked - converting to error"
                        );
                        Err(ParallelStateRootError::Other(
                            "account multiproof task panicked".into(),
                        ))
                    }
                };

                if result_sender.send(final_result).is_err() {
                    warn!(
                        target: "trie::proof_pool",
                        worker_id,
                        "Account multiproof result discarded - receiver dropped"
                    );
                }
            }

            debug!(target: "trie::proof_pool", worker_id, "Account proof worker shutdown");
        });
    }

    // Create and return handle directly
    Ok(ProofTaskManagerHandle::new(
        storage_work_tx,
        account_work_tx,
        Arc::new(AtomicUsize::new(0)),
        #[cfg(feature = "metrics")]
        storage_queue_depth,
        #[cfg(feature = "metrics")]
        account_queue_depth,
        #[cfg(feature = "metrics")]
        Arc::new(metrics),
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

    /// Identifier for the tx within the worker pool, used only for tracing.
    #[allow(dead_code)]
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
    pub result_sender: CrossbeamSender<Result<DecodedMultiProof, ParallelStateRootError>>,
}

impl<Tx> AccountMultiproofInput<Tx> {
    /// Creates a new [`AccountMultiproofInput`] with the given parameters.
    pub const fn new(
        targets: MultiProofTargets,
        prefix_sets: TriePrefixSets,
        collect_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
        storage_proof_handle: ProofTaskManagerHandle<Tx>,
        result_sender: CrossbeamSender<Result<DecodedMultiProof, ParallelStateRootError>>,
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

/// Proof task kind dispatched via [`ProofTaskManagerHandle::queue_task`].
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

/// A handle that wraps crossbeam senders for direct worker communication.
/// Channels are closed when the last handle is dropped.
#[derive(Debug)]
pub struct ProofTaskManagerHandle<Tx> {
    /// Direct sender to storage worker pool (handles storage proofs + blinded nodes)
    storage_work_tx: CrossbeamSender<ProofJob<Tx>>,
    /// Direct sender to account worker pool (handles account multiproofs)
    account_work_tx: CrossbeamSender<AccountProofJob<Tx>>,
    /// The number of active handles.
    active_handles: Arc<AtomicUsize>,
    /// Queue depth metrics
    #[cfg(feature = "metrics")]
    storage_queue_depth: Arc<AtomicUsize>,
    #[cfg(feature = "metrics")]
    account_queue_depth: Arc<AtomicUsize>,
    /// Metrics for recording on shutdown
    #[cfg(feature = "metrics")]
    metrics: Arc<ProofTaskMetrics>,
}

impl<Tx> ProofTaskManagerHandle<Tx> {
    /// Creates a new [`ProofTaskManagerHandle`] with crossbeam senders.
    pub fn new(
        storage_work_tx: CrossbeamSender<ProofJob<Tx>>,
        account_work_tx: CrossbeamSender<AccountProofJob<Tx>>,
        active_handles: Arc<AtomicUsize>,
        #[cfg(feature = "metrics")] storage_queue_depth: Arc<AtomicUsize>,
        #[cfg(feature = "metrics")] account_queue_depth: Arc<AtomicUsize>,
        #[cfg(feature = "metrics")] metrics: Arc<ProofTaskMetrics>,
    ) -> Self {
        active_handles.fetch_add(1, Ordering::SeqCst);
        Self {
            storage_work_tx,
            account_work_tx,
            active_handles,
            #[cfg(feature = "metrics")]
            storage_queue_depth,
            #[cfg(feature = "metrics")]
            account_queue_depth,
            #[cfg(feature = "metrics")]
            metrics,
        }
    }

    /// Queues a task directly to the appropriate worker pool.
    pub fn queue_task(&self, task: ProofTaskKind<Tx>) -> Result<(), String> {
        match task {
            ProofTaskKind::StorageProof(input, result_sender) => {
                #[cfg(feature = "metrics")]
                let old_depth = self.storage_queue_depth.fetch_add(1, Ordering::Relaxed);

                let job = ProofJob::StorageProof {
                    input,
                    result_tx: result_sender,
                    #[cfg(feature = "metrics")]
                    enqueued_at: Instant::now(),
                    _phantom: std::marker::PhantomData,
                };

                self.storage_work_tx.send(job).map_err(|_| {
                    #[cfg(feature = "metrics")]
                    self.storage_queue_depth.fetch_sub(1, Ordering::Relaxed);
                    "storage worker pool closed".to_string()
                })?;

                #[cfg(feature = "metrics")]
                {
                    // Record queue depth metric
                    let _ = old_depth;
                }

                Ok(())
            }
            ProofTaskKind::BlindedAccountNode(path, result_sender) => {
                let job = ProofJob::BlindedAccountNode {
                    path,
                    result_tx: result_sender,
                    _phantom: std::marker::PhantomData,
                };

                self.storage_work_tx.send(job).map_err(|_| "storage worker pool closed".to_string())
            }
            ProofTaskKind::BlindedStorageNode(account, path, result_sender) => {
                let job = ProofJob::BlindedStorageNode {
                    account,
                    path,
                    result_tx: result_sender,
                    _phantom: std::marker::PhantomData,
                };

                self.storage_work_tx.send(job).map_err(|_| "storage worker pool closed".to_string())
            }
            ProofTaskKind::AccountMultiproof(input) => {
                #[cfg(feature = "metrics")]
                let old_depth = self.account_queue_depth.fetch_add(1, Ordering::Relaxed);

                let job = AccountProofJob {
                    input: *input,
                    #[cfg(feature = "metrics")]
                    enqueued_at: Instant::now(),
                };

                self.account_work_tx.send(job).map_err(|_| {
                    #[cfg(feature = "metrics")]
                    self.account_queue_depth.fetch_sub(1, Ordering::Relaxed);
                    "account worker pool closed".to_string()
                })?;

                #[cfg(feature = "metrics")]
                {
                    let _ = old_depth;
                }

                Ok(())
            }
        }
    }
}

impl<Tx> Clone for ProofTaskManagerHandle<Tx> {
    fn clone(&self) -> Self {
        Self::new(
            self.storage_work_tx.clone(),
            self.account_work_tx.clone(),
            self.active_handles.clone(),
            #[cfg(feature = "metrics")]
            self.storage_queue_depth.clone(),
            #[cfg(feature = "metrics")]
            self.account_queue_depth.clone(),
            #[cfg(feature = "metrics")]
            self.metrics.clone(),
        )
    }
}

impl<Tx> Drop for ProofTaskManagerHandle<Tx> {
    fn drop(&mut self) {
        // Decrement the number of active handles and record metrics if this is the last one
        if self.active_handles.fetch_sub(1, Ordering::SeqCst) == 1 {
            #[cfg(feature = "metrics")]
            self.metrics.record();
        }
        // When last handle drops, crossbeam channels close automatically.
    }
}

impl<Tx: DbTx> TrieNodeProviderFactory for ProofTaskManagerHandle<Tx> {
    type AccountNodeProvider = ProofTaskTrieNodeProvider<Tx>;
    type StorageNodeProvider = ProofTaskTrieNodeProvider<Tx>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { sender: self.storage_work_tx.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, sender: self.storage_work_tx.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
/// Sends blinded node requests directly to storage workers.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider<Tx> {
    /// Blinded account trie node provider.
    AccountNode {
        /// Direct sender to storage worker pool
        sender: CrossbeamSender<ProofJob<Tx>>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Direct sender to storage worker pool
        sender: CrossbeamSender<ProofJob<Tx>>,
    },
}

impl<Tx: DbTx> TrieNodeProvider for ProofTaskTrieNodeProvider<Tx> {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let (tx, rx) = crossbeam_channel::unbounded();

        match self {
            Self::AccountNode { sender } => {
                let job = ProofJob::BlindedAccountNode {
                    path: *path,
                    result_tx: tx,
                    _phantom: std::marker::PhantomData,
                };
                let _ = sender.send(job);
            }
            Self::StorageNode { sender, account } => {
                let job = ProofJob::BlindedStorageNode {
                    account: *account,
                    path: *path,
                    result_tx: tx,
                    _phantom: std::marker::PhantomData,
                };
                let _ = sender.send(job);
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
    fn spawn_proof_workers_propagates_consistent_view_error() {
        let factory = create_test_provider_factory();
        let view = ConsistentDbView::new(factory, Some((B256::repeat_byte(0x11), 0)));

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();

        let err = spawn_proof_workers(rt.handle().clone(), view, task_ctx, 1, 1, 1).unwrap_err();

        assert!(matches!(
            err,
            ProviderError::ConsistentView(ref e) if matches!(**e, ConsistentViewError::Reorged { .. })
        ));
    }

    #[test]
    fn spawn_proof_workers_creates_requested_workers_and_processes_tasks() {
        let inner_factory = create_test_provider_factory();
        let calls = Arc::new(AtomicUsize::new(0));
        let counting_factory = CountingFactory::new(inner_factory, Arc::clone(&calls));
        let view = ConsistentDbView::new(counting_factory, None);

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();
        let storage_workers = 2usize;
        let account_workers = 1usize;
        let handle = spawn_proof_workers(
            rt.handle().clone(),
            view,
            task_ctx,
            storage_workers,
            account_workers,
            4,
        )
        .unwrap();

        let expected_total_workers = storage_workers + account_workers;
        assert_eq!(calls.load(Ordering::SeqCst), expected_total_workers);

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
    }

    /// Tests that storage workers reuse the same database transaction across multiple proofs,
    /// validating the core Phase 1a optimization that eliminates per-proof transaction overhead.
    #[test]
    fn storage_worker_reuses_transaction_across_multiple_proofs() {
        let inner_factory = create_test_provider_factory();
        let calls = Arc::new(AtomicUsize::new(0));
        let counting_factory = CountingFactory::new(inner_factory, Arc::clone(&calls));
        let view = ConsistentDbView::new(counting_factory, None);

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();
        let storage_workers = 1usize;
        let account_workers = 0usize; // Gets clamped to 1 inside spawn_proof_workers
        let handle = spawn_proof_workers(
            rt.handle().clone(),
            view,
            task_ctx,
            storage_workers,
            account_workers,
            4,
        )
        .unwrap();

        // Worker counts are clamped to min 1 inside spawn_proof_workers
        // Expect 2 transactions: 1 for storage worker + 1 for account worker (clamped from 0)
        let initial_calls = calls.load(Ordering::SeqCst);
        assert_eq!(initial_calls, 2);

        // Queue 10 storage proofs - all should use same transaction
        let prefix_set = PrefixSetMut::default().freeze();
        let mut receivers = Vec::new();
        for _ in 0..10 {
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
            let _ = receiver.recv().unwrap();
        }

        // Transaction count should still be 2 (workers reuse their transactions)
        assert_eq!(calls.load(Ordering::SeqCst), initial_calls);

        drop(handle);
    }

    /// Tests that the worker architecture handles heavy concurrent load without deadlocks,
    /// validating unbounded channel backpressure behavior under stress.
    #[test]
    fn handles_backpressure_with_many_concurrent_storage_proofs() {
        let inner_factory = create_test_provider_factory();
        let view = ConsistentDbView::new(inner_factory, None);

        let rt = Runtime::new().unwrap();
        let task_ctx = default_task_ctx();
        // 2 storage workers + 1 account worker = 3 total workers
        let handle = spawn_proof_workers(rt.handle().clone(), view, task_ctx, 2, 1, 4).unwrap();

        // Queue 50 storage proofs concurrently
        let prefix_set = PrefixSetMut::default().freeze();
        let mut receivers = Vec::new();
        for _ in 0..50 {
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

        // All tasks complete without deadlock
        for receiver in receivers {
            let _ = receiver.recv().unwrap();
        }

        drop(handle);
    }
}
