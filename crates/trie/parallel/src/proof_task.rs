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
//! # Message Flow
//!
//! 1. [`MultiProofTask`](reth_engine_tree::tree::payload_processor::multiproof::MultiProofTask)
//!    prepares a storage or account job and hands it to [`ProofWorkerHandle`]. The job carries a
//!    [`ProofResultContext`] so the worker knows how to send the result back.
//! 2. A worker receives the job, runs the proof, and sends a [`ProofResultMessage`] through the
//!    provided [`ProofResultSender`].
//! 3. `MultiProofTask` receives the message, uses `sequence_number` to keep proofs in order, and
//!    proceeds with its state-root logic.
//!
//! Each job gets its own direct channel so results go straight back to `MultiProofTask`. That keeps
//! ordering decisions in one place and lets workers run independently.
//!
//! ```text
//! MultiProofTask -> MultiproofManager -> ProofWorkerHandle -> Storage/Account Worker
//!        ^                                          |
//!        |                                          v
//! ProofResultMessage <-------- ProofResultSender ---
//! ```
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
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostState,
    HashedPostStateSorted, MultiProofTargets, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
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
    time::{Duration, Instant},
};
use tokio::runtime::Handle;
use tracing::{debug_span, error, trace};

#[cfg(feature = "metrics")]
use crate::proof_task_metrics::ProofTaskTrieMetrics;

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;
type AccountMultiproofResult =
    Result<(DecodedMultiProof, ParallelTrieStats), ParallelStateRootError>;

/// Channel used by worker threads to deliver `ProofResultMessage` items back to
/// `MultiProofTask`.
///
/// Workers use this sender to deliver proof results directly to `MultiProofTask`.
pub type ProofResultSender = CrossbeamSender<ProofResultMessage>;

/// Context for sending proof calculation results back to `MultiProofTask`.
///
/// This struct contains all context needed to send and track proof calculation results.
/// Workers use this to deliver completed proofs back to the main event loop.
#[derive(Debug, Clone)]
pub struct ProofResultContext {
    /// Channel sender for result delivery
    pub sender: ProofResultSender,
    /// Sequence number for proof ordering
    pub sequence_number: u64,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
    /// Calculation start time for measuring elapsed duration
    pub start_time: Instant,
}

impl ProofResultContext {
    /// Creates a new proof result context.
    pub const fn new(
        sender: ProofResultSender,
        sequence_number: u64,
        state: HashedPostState,
        start_time: Instant,
    ) -> Self {
        Self { sender, sequence_number, state, start_time }
    }
}

/// Message containing a completed proof result with metadata for direct delivery to
/// `MultiProofTask`.
///
/// This type enables workers to send proof results directly to the `MultiProofTask` event loop.
#[derive(Debug)]
pub struct ProofResultMessage {
    /// Sequence number for ordering proofs
    pub sequence_number: u64,
    /// The proof calculation result
    pub result: AccountMultiproofResult,
    /// Time taken for the entire proof calculation (from dispatch to completion)
    pub elapsed: Duration,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
}

/// Internal message for storage workers.
#[derive(Debug)]
enum StorageWorkerJob {
    /// Storage proof computation request
    StorageProof {
        /// Storage proof input parameters
        input: StorageProofInput,
        /// Context for sending the proof result.
        proof_result_sender: ProofResultContext,
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
    available_workers: Arc<AtomicUsize>,
    #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
) where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    // Create db transaction before entering work loop
    let provider =
        view.provider_ro().expect("Storage worker failed to initialize: database unavailable");
    let proof_tx = ProofTaskTx::new(provider.into_tx(), task_ctx, worker_id);

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

    // Initially mark this worker as available.
    available_workers.fetch_add(1, Ordering::Relaxed);

    while let Ok(job) = work_rx.recv() {
        // Mark worker as busy.
        available_workers.fetch_sub(1, Ordering::Relaxed);

        match job {
            StorageWorkerJob::StorageProof { input, proof_result_sender } => {
                let hashed_address = input.hashed_address;
                let ProofResultContext { sender, sequence_number: seq, state, start_time } =
                    proof_result_sender;

                trace!(
                    target: "trie::proof_task",
                    worker_id,
                    hashed_address = ?hashed_address,
                    prefix_set_len = input.prefix_set.len(),
                    target_slots_len = input.target_slots.len(),
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

                // Convert storage proof to account multiproof format
                let result_msg = match result {
                    Ok(storage_proof) => {
                        let multiproof = reth_trie::DecodedMultiProof::from_storage_proof(
                            hashed_address,
                            storage_proof,
                        );
                        let stats = crate::stats::ParallelTrieTracker::default().finish();
                        Ok((multiproof, stats))
                    }
                    Err(e) => Err(e),
                };

                // Send result directly to MultiProofTask
                if sender
                    .send(ProofResultMessage {
                        sequence_number: seq,
                        result: result_msg,
                        elapsed: start_time.elapsed(),
                        state,
                    })
                    .is_err()
                {
                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        hashed_address = ?hashed_address,
                        storage_proofs_processed,
                        "Proof result receiver dropped, discarding result"
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

                // Mark worker as available again.
                available_workers.fetch_add(1, Ordering::Relaxed);
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

                // Mark worker as available again.
                available_workers.fetch_add(1, Ordering::Relaxed);
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
/// Each worker initializes its providers, advertises availability, then loops:
/// take a job, mark busy, compute the proof, send the result, and mark available again.
/// The loop ends gracefully once the channel closes.
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
    available_workers: Arc<AtomicUsize>,
    #[cfg(feature = "metrics")] metrics: ProofTaskTrieMetrics,
) where
    Factory: DatabaseProviderFactory<Provider: BlockReader>,
{
    // Create db transaction before entering work loop
    let provider =
        view.provider_ro().expect("Account worker failed to initialize: database unavailable");
    let proof_tx = ProofTaskTx::new(provider.into_tx(), task_ctx, worker_id);

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

    // Count this worker as available only after successful initialization.
    available_workers.fetch_add(1, Ordering::Relaxed);

    while let Ok(job) = work_rx.recv() {
        // Mark worker as busy.
        available_workers.fetch_sub(1, Ordering::Relaxed);

        match job {
            AccountWorkerJob::AccountMultiproof { input } => {
                let AccountMultiproofInput {
                    targets,
                    mut prefix_sets,
                    collect_branch_node_masks,
                    multi_added_removed_keys,
                    missed_leaves_storage_roots,
                    proof_result_sender:
                        ProofResultContext {
                            sender: result_tx,
                            sequence_number: seq,
                            state,
                            start_time: start,
                        },
                } = *input;

                let span = tracing::debug_span!(
                    target: "trie::proof_task",
                    "Account multiproof calculation",
                    targets = targets.len(),
                    worker_id,
                );
                let _span_guard = span.enter();

                trace!(
                    target: "trie::proof_task",
                    "Processing account multiproof"
                );

                let proof_start = Instant::now();

                let mut tracker = ParallelTrieTracker::default();

                let mut storage_prefix_sets = std::mem::take(&mut prefix_sets.storage_prefix_sets);

                let storage_root_targets_len = StorageRootTargets::count(
                    &prefix_sets.account_prefix_set,
                    &storage_prefix_sets,
                );

                tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

                let storage_proof_receivers = match dispatch_storage_proofs(
                    &storage_work_tx,
                    &targets,
                    &mut storage_prefix_sets,
                    collect_branch_node_masks,
                    multi_added_removed_keys.as_ref(),
                ) {
                    Ok(receivers) => receivers,
                    Err(error) => {
                        // Send error through result channel
                        error!(target: "trie::proof_task", "Failed to dispatch storage proofs: {error}");
                        let _ = result_tx.send(ProofResultMessage {
                            sequence_number: seq,
                            result: Err(error),
                            elapsed: start.elapsed(),
                            state,
                        });
                        continue;
                    }
                };

                // Use the missed leaves cache passed from the multiproof manager
                let account_prefix_set = std::mem::take(&mut prefix_sets.account_prefix_set);

                let ctx = AccountMultiproofParams {
                    targets: &targets,
                    prefix_set: account_prefix_set,
                    collect_branch_node_masks,
                    multi_added_removed_keys: multi_added_removed_keys.as_ref(),
                    storage_proof_receivers,
                    missed_leaves_storage_roots: missed_leaves_storage_roots.as_ref(),
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

                // Send result to MultiProofTask
                if result_tx
                    .send(ProofResultMessage {
                        sequence_number: seq,
                        result,
                        elapsed: proof_elapsed,
                        state,
                    })
                    .is_err()
                {
                    tracing::debug!(
                        target: "trie::proof_task",
                        worker_id,
                        account_proofs_processed,
                        "Account multiproof receiver dropped, discarding result"
                    );
                }

                trace!(
                    target: "trie::proof_task",
                    proof_time_us = proof_elapsed.as_micros(),
                    total_processed = account_proofs_processed,
                    "Account multiproof completed"
                );
                drop(_span_guard);

                // Mark worker as available again.
                available_workers.fetch_add(1, Ordering::Relaxed);
            }

            AccountWorkerJob::BlindedAccountNode { path, result_sender } => {
                let span = tracing::debug_span!(
                    target: "trie::proof_task",
                    "Blinded account node calculation",
                    ?path,
                    worker_id,
                );
                let _span_guard = span.enter();

                trace!(
                    target: "trie::proof_task",
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
                    node_time_us = elapsed.as_micros(),
                    total_processed = account_nodes_processed,
                    "Blinded account node completed"
                );
                drop(_span_guard);

                // Mark worker as available again.
                available_workers.fetch_add(1, Ordering::Relaxed);
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
                        let proof_msg = receiver.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(
                                reth_execution_errors::StorageRootError::Database(
                                    DatabaseError::Other(format!(
                                        "Storage proof channel closed for {hashed_address}"
                                    )),
                                ),
                            )
                        })?;

                        // Extract storage proof from the multiproof wrapper
                        let (mut multiproof, _stats) = proof_msg.result?;
                        let proof =
                            multiproof.storages.remove(&hashed_address).ok_or_else(|| {
                                ParallelStateRootError::Other(format!(
                                    "storage proof not found in multiproof for {hashed_address}"
                                ))
                            })?;

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
        if let Ok(proof_msg) = receiver.recv() {
            // Extract storage proof from the multiproof wrapper
            if let Ok((mut multiproof, _stats)) = proof_msg.result &&
                let Some(proof) = multiproof.storages.remove(&hashed_address)
            {
                collected_decoded_storages.insert(hashed_address, proof);
            }
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
) -> Result<B256Map<CrossbeamReceiver<ProofResultMessage>>, ParallelStateRootError> {
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());

    // Dispatch all storage proofs to worker pool
    for (hashed_address, target_slots) in targets.iter() {
        let prefix_set = storage_prefix_sets.remove(hashed_address).unwrap_or_default();

        // Create channel for receiving ProofResultMessage
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let start = Instant::now();

        // Create computation input (data only, no communication channel)
        let input = StorageProofInput::new(
            *hashed_address,
            prefix_set,
            target_slots.clone(),
            with_branch_node_masks,
            multi_added_removed_keys.cloned(),
        );

        // Always dispatch a storage proof so we obtain the storage root even when no slots are
        // requested.
        storage_work_tx
            .send(StorageWorkerJob::StorageProof {
                input,
                proof_result_sender: ProofResultContext::new(
                    result_tx,
                    0,
                    HashedPostState::default(),
                    start,
                ),
            })
            .map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "Failed to queue storage proof for {}: storage worker pool unavailable",
                    hashed_address
                ))
            })?;

        storage_proof_receivers.insert(*hashed_address, result_rx);
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

        let span = tracing::debug_span!(
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

/// Input parameters for storage proof computation.
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
    /// Context for sending the proof result.
    pub proof_result_sender: ProofResultContext,
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
    storage_proof_receivers: B256Map<CrossbeamReceiver<ProofResultMessage>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    missed_leaves_storage_roots: &'a DashMap<B256, B256>,
}

/// Internal message for account workers.
#[derive(Debug)]
enum AccountWorkerJob {
    /// Account multiproof computation request
    AccountMultiproof {
        /// Account multiproof input parameters
        input: Box<AccountMultiproofInput>,
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
    /// Counter tracking available storage workers. Workers decrement when starting work,
    /// increment when finishing. Used to determine whether to chunk multiproofs.
    storage_available_workers: Arc<AtomicUsize>,
    /// Counter tracking available account workers. Workers decrement when starting work,
    /// increment when finishing. Used to determine whether to chunk multiproofs.
    account_available_workers: Arc<AtomicUsize>,
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

        // Initialize availability counters at zero. Each worker will increment when it
        // successfully initializes, ensuring only healthy workers are counted.
        let storage_available_workers = Arc::new(AtomicUsize::new(0));
        let account_available_workers = Arc::new(AtomicUsize::new(0));

        tracing::debug!(
            target: "trie::proof_task",
            storage_worker_count,
            account_worker_count,
            "Spawning proof worker pools"
        );

        let storage_worker_parent =
            debug_span!(target: "trie::proof_task", "Storage worker tasks", ?storage_worker_count);
        let _guard = storage_worker_parent.enter();

        // Spawn storage workers
        for worker_id in 0..storage_worker_count {
            let parent_span = debug_span!(target: "trie::proof_task", "Storage worker", ?worker_id);
            let view_clone = view.clone();
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = storage_work_rx.clone();
            let storage_available_workers_clone = storage_available_workers.clone();

            executor.spawn_blocking(move || {
                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();

                let _guard = parent_span.enter();
                storage_worker_loop(
                    view_clone,
                    task_ctx_clone,
                    work_rx_clone,
                    worker_id,
                    storage_available_workers_clone,
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

        drop(_guard);

        let account_worker_parent =
            debug_span!(target: "trie::proof_task", "Account worker tasks", ?account_worker_count);
        let _guard = account_worker_parent.enter();

        // Spawn account workers
        for worker_id in 0..account_worker_count {
            let parent_span = debug_span!(target: "trie::proof_task", "Account worker", ?worker_id);
            let view_clone = view.clone();
            let task_ctx_clone = task_ctx.clone();
            let work_rx_clone = account_work_rx.clone();
            let storage_work_tx_clone = storage_work_tx.clone();
            let account_available_workers_clone = account_available_workers.clone();

            executor.spawn_blocking(move || {
                #[cfg(feature = "metrics")]
                let metrics = ProofTaskTrieMetrics::default();

                let _guard = parent_span.enter();
                account_worker_loop(
                    view_clone,
                    task_ctx_clone,
                    work_rx_clone,
                    storage_work_tx_clone,
                    worker_id,
                    account_available_workers_clone,
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

        drop(_guard);

        Self::new_handle(
            storage_work_tx,
            account_work_tx,
            storage_available_workers,
            account_available_workers,
        )
    }

    /// Creates a new [`ProofWorkerHandle`] with direct access to worker pools.
    ///
    /// This is an internal constructor used for creating handles.
    const fn new_handle(
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        account_work_tx: CrossbeamSender<AccountWorkerJob>,
        storage_available_workers: Arc<AtomicUsize>,
        account_available_workers: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            storage_work_tx,
            account_work_tx,
            storage_available_workers,
            account_available_workers,
        }
    }

    /// Returns true if there are available storage workers to process tasks.
    pub fn has_available_storage_workers(&self) -> bool {
        self.storage_available_workers.load(Ordering::Relaxed) > 0
    }

    /// Returns true if there are available account workers to process tasks.
    pub fn has_available_account_workers(&self) -> bool {
        self.account_available_workers.load(Ordering::Relaxed) > 0
    }

    /// Returns the number of pending storage tasks in the queue.
    pub fn pending_storage_tasks(&self) -> usize {
        self.storage_work_tx.len()
    }

    /// Returns the number of pending account tasks in the queue.
    pub fn pending_account_tasks(&self) -> usize {
        self.account_work_tx.len()
    }

    /// Dispatch a storage proof computation to storage worker pool
    ///
    /// The result will be sent via the `proof_result_sender` channel.
    pub fn dispatch_storage_proof(
        &self,
        input: StorageProofInput,
        proof_result_sender: ProofResultContext,
    ) -> Result<(), ProviderError> {
        self.storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("storage workers unavailable"));

                if let StorageWorkerJob::StorageProof { proof_result_sender, .. } = err.0 {
                    let ProofResultContext {
                        sender: result_tx,
                        sequence_number: seq,
                        state,
                        start_time: start,
                    } = proof_result_sender;

                    let _ = result_tx.send(ProofResultMessage {
                        sequence_number: seq,
                        result: Err(ParallelStateRootError::Provider(error.clone())),
                        elapsed: start.elapsed(),
                        state,
                    });
                }

                error
            })
    }

    /// Dispatch an account multiproof computation
    ///
    /// The result will be sent via the `result_sender` channel included in the input.
    pub fn dispatch_account_multiproof(
        &self,
        input: AccountMultiproofInput,
    ) -> Result<(), ProviderError> {
        self.account_work_tx
            .send(AccountWorkerJob::AccountMultiproof { input: Box::new(input) })
            .map_err(|err| {
                let error =
                    ProviderError::other(std::io::Error::other("account workers unavailable"));

                if let AccountWorkerJob::AccountMultiproof { input } = err.0 {
                    let AccountMultiproofInput {
                        proof_result_sender:
                            ProofResultContext {
                                sender: result_tx,
                                sequence_number: seq,
                                state,
                                start_time: start,
                            },
                        ..
                    } = *input;

                    let _ = result_tx.send(ProofResultMessage {
                        sequence_number: seq,
                        result: Err(ParallelStateRootError::Provider(error.clone())),
                        elapsed: start.elapsed(),
                        state,
                    });
                }

                error
            })
    }

    /// Dispatch blinded storage node request to storage worker pool
    pub(crate) fn dispatch_blinded_storage_node(
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

    /// Dispatch blinded account node request to account worker pool
    pub(crate) fn dispatch_blinded_account_node(
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
                    .dispatch_blinded_account_node(*path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
            Self::StorageNode { handle, account } => {
                let rx = handle
                    .dispatch_blinded_storage_node(*account, *path)
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
