//! Parallel proof computation using worker pools with dedicated database transactions.
//!
//!
//! # Architecture
//!
//! - **Worker Pools**: Pre-spawned workers with dedicated database transactions
//!   - Storage pool: Handles storage proofs and blinded storage node requests
//!   - Account pool: Handles account multiproofs and blinded account node requests
//! - **Direct Channel Access**: [`ProofWorkerHandle`] provides type-safe queue methods with direct
//!   access to worker channels, eliminating routing overhead
//! - **Automatic Shutdown**: Workers terminate gracefully when all handles are dropped
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
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
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
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};
use tokio::runtime::Handle;
use tracing::trace;

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskTrieMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;
type AccountMultiproofResult =
    Result<(DecodedMultiProof, ParallelTrieStats), ParallelStateRootError>;

/// Internal message for storage workers.
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
fn storage_worker_loop<Factory>(
    view: ConsistentDbView<Factory>,
    task_ctx: ProofTaskCtx,
    work_rx: CrossbeamReceiver<StorageWorkerJob>,
    worker_id: usize,
    #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
) where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    // Create db transaction before entering work loop
    let proof_tx = match view.provider_ro() {
        Ok(provider) => {
            let tx = provider.into_tx();
            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                "Storage worker initialized transaction successfully"
            );
            ProofTaskTx::new(tx, task_ctx, worker_id)
        }
        Err(e) => {
            tracing::error!(
                target: "trie::proof_task",
                worker_id,
                error = %e,
                "Storage worker failed to initialize transaction, exiting"
            );
            return;
        }
    };

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

    #[cfg(feature = "metrics")]
    metrics.record_storage_nodes(storage_nodes_processed as usize);
}

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
fn account_worker_loop<Factory>(
    view: ConsistentDbView<Factory>,
    task_ctx: ProofTaskCtx,
    work_rx: CrossbeamReceiver<AccountWorkerJob>,
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    worker_id: usize,
    #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
) where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    // Create db transaction before entering work loop
    let proof_tx = match view.provider_ro() {
        Ok(provider) => {
            let tx = provider.into_tx();
            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                "Account worker initialized transaction successfully"
            );
            ProofTaskTx::new(tx, task_ctx, worker_id)
        }
        Err(e) => {
            tracing::error!(
                target: "trie::proof_task",
                worker_id,
                error = %e,
                "Account worker failed to initialize transaction, exiting"
            );
            return;
        }
    };

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

                let storage_proof_receivers = match dispatch_storage_proofs(
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

                let ctx = AccountMultiproofParams {
                    targets: &input.targets,
                    prefix_set: account_prefix_set,
                    collect_branch_node_masks: input.collect_branch_node_masks,
                    multi_added_removed_keys: input.multi_added_removed_keys.as_ref(),
                    storage_proof_receivers,
                    missed_leaves_storage_roots,
                };

                let result = build_account_multiproof_with_storage_roots(
                    trie_cursor_factory.clone(),
                    hashed_cursor_factory.clone(),
                    ctx,
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

    #[cfg(feature = "metrics")]
    metrics.record_account_nodes(account_nodes_processed as usize);
}

/// Builds an account multiproof by consuming storage proof receivers lazily during trie walk.
///
/// This is a helper function used by account workers to build the account subtree proof
/// while storage proofs are still being computed. Receivers are consumed only when needed,
/// enabling interleaved parallelism between account trie traversal and storage proof computation.
///
/// Returns a `DecodedMultiProof` containing the account subtree and storage proofs.
fn build_account_multiproof_with_storage_roots<C, H>(
    trie_cursor_factory: C,
    hashed_cursor_factory: H,
    ctx: AccountMultiproofParams<'_>,
    tracker: &mut ParallelTrieTracker,
) -> Result<DecodedMultiProof, ParallelStateRootError>
where
    C: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    let accounts_added_removed_keys =
        ctx.multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

    // Create the walker.
    let walker = TrieWalker::<_>::state_trie(
        trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
        ctx.prefix_set,
    )
    .with_added_removed_keys(accounts_added_removed_keys)
    .with_deletions_retained(true);

    // Create a hash builder to rebuild the root node since it is not available in the database.
    let retainer = ctx
        .targets
        .keys()
        .map(Nibbles::unpack)
        .collect::<ProofRetainer>()
        .with_added_removed_keys(accounts_added_removed_keys);
    let mut hash_builder = HashBuilder::default()
        .with_proof_retainer(retainer)
        .with_updates(ctx.collect_branch_node_masks);

    // Initialize storage multiproofs map with pre-allocated capacity.
    // Proofs will be inserted as they're consumed from receivers during trie walk.
    let mut collected_decoded_storages: B256Map<DecodedStorageMultiProof> =
        B256Map::with_capacity_and_hasher(ctx.targets.len(), Default::default());
    let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    let mut account_node_iter = TrieNodeIter::state_trie(
        walker,
        hashed_cursor_factory.hashed_account_cursor().map_err(ProviderError::Database)?,
    );

    let mut storage_proof_receivers = ctx.storage_proof_receivers;

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

                        match ctx.missed_leaves_storage_roots.entry(hashed_address) {
                            dashmap::Entry::Occupied(occ) => *occ.get(),
                            dashmap::Entry::Vacant(vac) => {
                                let root = StorageProof::new_hashed(
                                    trie_cursor_factory.clone(),
                                    hashed_cursor_factory.clone(),
                                    hashed_address,
                                )
                                .with_prefix_set_mut(Default::default())
                                .storage_multiproof(
                                    ctx.targets.get(&hashed_address).cloned().unwrap_or_default(),
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

    let (branch_node_hash_masks, branch_node_tree_masks) = if ctx.collect_branch_node_masks {
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
fn dispatch_storage_proofs(
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

    /// Identifier for the worker within the worker pool, used only for tracing.
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

    /// Compute storage proof with pre-created factories.
    ///
    /// Accepts cursor factories as parameters to allow reuse across multiple proofs.
    /// Used by storage workers in the worker pool to avoid factory recreation
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

/// Parameters for building an account multiproof with pre-computed storage roots.
struct AccountMultiproofParams<'a> {
    /// The targets for which to compute the multiproof.
    targets: &'a MultiProofTargets,
    /// The prefix set for the account trie walk.
    prefix_set: PrefixSet,
    /// Whether or not to collect branch node masks.
    collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<&'a Arc<MultiAddedRemovedKeys>>,
    /// Receivers for storage proofs being computed in parallel.
    storage_proof_receivers: B256Map<Receiver<StorageProofResult>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    missed_leaves_storage_roots: &'a DashMap<B256, B256>,
}

/// Internal message for account workers.
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

/// A handle that provides type-safe access to proof worker pools.
///
/// The handle stores direct senders to both storage and account worker pools,
/// eliminating the need for a routing thread. All handles share reference-counted
/// channels, and workers shut down gracefully when all handles are dropped.
#[derive(Debug, Clone)]
pub struct ProofWorkerHandle {
    /// Direct sender to storage worker pool
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Direct sender to account worker pool
    account_work_tx: CrossbeamSender<AccountWorkerJob>,
}

impl ProofWorkerHandle {
    /// Spawns storage and account worker pools with dedicated database transactions.
    ///
    /// Returns a handle for submitting proof tasks to the worker pools.
    /// Workers run until the last handle is dropped.
    ///
    /// # Parameters
    /// - `executor`: Tokio runtime handle for spawning blocking tasks
    /// - `view`: Consistent database view for creating transactions
    /// - `task_ctx`: Shared context with trie updates and prefix sets
    /// - `storage_worker_count`: Number of storage workers to spawn
    /// - `account_worker_count`: Number of account workers to spawn
    pub fn new<Factory>(
        executor: Handle,
        view: ConsistentDbView<Factory>,
        task_ctx: ProofTaskCtx,
        storage_worker_count: usize,
        account_worker_count: usize,
    ) -> Self
    where
        Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static,
    {
        let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();
        let (account_work_tx, account_work_rx) = unbounded::<AccountWorkerJob>();

        tracing::debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            "Spawning proof worker pools"
        );

        // Spawn storage workers
        for worker_id in 0..storage_worker_count {
            let view_clone = view.clone();
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = storage_work_rx.clone();

            executor.spawn_blocking(move || {
                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();

                storage_worker_loop(
                    view_clone,
                    task_ctx_clone,
                    work_rx_clone,
                    worker_id,
                    #[cfg(feature = "metrics")]
                    metrics,
                )
            });

            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                "Storage worker spawned successfully"
            );
        }

        // Spawn account workers
        for worker_id in 0..account_worker_count {
            let view_clone = view.clone();
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = account_work_rx.clone();
            let storage_work_tx_clone = storage_work_tx.clone();

            executor.spawn_blocking(move || {
                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();

                account_worker_loop(
                    view_clone,
                    task_ctx_clone,
                    work_rx_clone,
                    storage_work_tx_clone,
                    worker_id,
                    #[cfg(feature = "metrics")]
                    metrics,
                )
            });

            tracing::debug!(
                target: "trie::proof_task",
                worker_id,
                "Account worker spawned successfully"
            );
        }

        Self::new_handle(storage_work_tx, account_work_tx)
    }

    /// Creates a new [`ProofWorkerHandle`] with direct access to worker pools.
    ///
    /// This is an internal constructor used for creating handles.
    const fn new_handle(
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        account_work_tx: CrossbeamSender<AccountWorkerJob>,
    ) -> Self {
        Self { storage_work_tx, account_work_tx }
    }

    /// Queue a storage proof computation
    pub fn queue_storage_proof(
        &self,
        input: StorageProofInput,
    ) -> Result<Receiver<StorageProofResult>, ProviderError> {
        let (tx, rx) = channel();
        self.storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("storage workers unavailable"))
            })?;

        Ok(rx)
    }

    /// Queue an account multiproof computation
    pub fn dispatch_account_multiproof(
        &self,
        input: AccountMultiproofInput,
    ) -> Result<Receiver<AccountMultiproofResult>, ProviderError> {
        let (tx, rx) = channel();
        self.account_work_tx
            .send(AccountWorkerJob::AccountMultiproof { input, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("account workers unavailable"))
            })?;

        Ok(rx)
    }

    /// Internal: Queue blinded storage node request
    fn queue_blinded_storage_node(
        &self,
        account: B256,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        self.storage_work_tx
            .send(StorageWorkerJob::BlindedStorageNode { account, path, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("storage workers unavailable"))
            })?;

        Ok(rx)
    }

    /// Internal: Queue blinded account node request
    fn queue_blinded_account_node(
        &self,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        self.account_work_tx
            .send(AccountWorkerJob::BlindedAccountNode { path, result_sender: tx })
            .map_err(|_| {
                ProviderError::other(std::io::Error::other("account workers unavailable"))
            })?;

        Ok(rx)
    }
}

impl TrieNodeProviderFactory for ProofWorkerHandle {
    type AccountNodeProvider = ProofTaskTrieNodeProvider;
    type StorageNodeProvider = ProofTaskTrieNodeProvider;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { handle: self.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, handle: self.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider {
    /// Blinded account trie node provider.
    AccountNode {
        /// Handle to the proof worker pools.
        handle: ProofWorkerHandle,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Handle to the proof worker pools.
        handle: ProofWorkerHandle,
    },
}

impl TrieNodeProvider for ProofTaskTrieNodeProvider {
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        match self {
            Self::AccountNode { handle } => {
                let rx = handle
                    .queue_blinded_account_node(*path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
            Self::StorageNode { handle, account } => {
                let rx = handle
                    .queue_blinded_storage_node(*account, *path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
        }
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

    /// Ensures `ProofWorkerHandle::new` spawns workers correctly.
    #[test]
    fn spawn_proof_workers_creates_handle() {
        let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = tokio::runtime::Handle::current();
            let factory = create_test_provider_factory();
            let view = ConsistentDbView::new(factory, None);
            let ctx = test_ctx();

            let proof_handle = ProofWorkerHandle::new(handle.clone(), view, ctx, 5, 3);

            // Verify handle can be cloned
            let _cloned_handle = proof_handle.clone();

            // Workers shut down automatically when handle is dropped
            drop(proof_handle);
            task::yield_now().await;
        });
    }
}
