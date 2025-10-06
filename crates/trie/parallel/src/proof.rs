use crate::{
    metrics::ParallelTrieMetrics,
    proof_task::{ProofTaskKind, ProofTaskManagerHandle, StorageProofInput, StorageProofResult},
    root::ParallelStateRootError,
    stats::{ParallelTrieStats, ParallelTrieTracker},
    StorageRootTargets,
};
use alloy_primitives::{
    map::{B256Map, B256Set, HashMap},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use crossbeam_channel::Receiver;
use dashmap::{mapref::entry::Entry, DashMap};
use itertools::Itertools;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderError,
};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSets, TriePrefixSetsMut},
    proof::StorageProof,
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    walker::TrieWalker,
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostStateSorted,
    MultiProofTargets, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    proof::{DecodedProofNodes, ProofRetainer},
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Builds an account multiproof with storage proof receivers.
///
/// This function accepts a map of storage proof receivers and fetches proofs on-demand
/// during account trie traversal, allowing account trie walking to interleave with
/// storage proof computation for better performance.
///
/// Returns a tuple containing the decoded multiproof and stats for metrics recording.
pub fn build_account_multiproof_with_storage<TCF, HCF>(
    trie_cursor_factory: TCF,
    hashed_cursor_factory: HCF,
    targets: MultiProofTargets,
    prefix_sets: TriePrefixSets,
    mut storage_receivers: B256Map<Receiver<StorageProofResult>>,
    collect_branch_node_masks: bool,
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
) -> Result<(DecodedMultiProof, ParallelTrieStats), ParallelStateRootError>
where
    TCF: TrieCursorFactory + Clone,
    HCF: HashedCursorFactory + Clone,
{
    debug!(
        target: "trie::proof",
        targets_count = targets.len(),
        prefix_sets_account_len = prefix_sets.account_prefix_set.len(),
        storage_receivers_count = storage_receivers.len(),
        "build_account_multiproof_with_storage starting"
    );

    let initial_storage_receivers = storage_receivers.len();

    // Track fallbacks to detect duplicates (proves cache would help)
    let mut fallback_tracker = std::collections::HashMap::<B256, usize>::new();

    let mut tracker = ParallelTrieTracker::default();

    let accounts_added_removed_keys =
        multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

    // Create the walker.
    let walker = TrieWalker::<_>::state_trie(
        trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
        prefix_sets.account_prefix_set,
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

    // Initialize all storage multiproofs as empty.
    // Storage multiproofs for non empty tries will be overwritten if necessary.
    let mut collected_decoded_storages: B256Map<DecodedStorageMultiProof> =
        targets.keys().map(|key| (*key, DecodedStorageMultiProof::empty())).collect();
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
                // Fetch storage proof on-demand (blocks if not yet ready)
                let decoded_storage_multiproof = match storage_receivers.remove(&hashed_address) {
                    Some(receiver) => {
                        // Try non-blocking receive first to track if proof was ready
                        match receiver.try_recv() {
                            Ok(Ok(proof)) => {
                                tracker.inc_storage_proof_immediate();
                                proof
                            }
                            Ok(Err(e)) => return Err(e),
                            Err(crossbeam_channel::TryRecvError::Empty) => {
                                // Proof not ready yet, block and wait
                                tracker.inc_storage_proof_blocked();
                                match receiver.recv() {
                                    Ok(Ok(proof)) => proof,
                                    Ok(Err(e)) => return Err(e),
                                    Err(_) => {
                                        return Err(storage_channel_closed_error(&hashed_address))
                                    }
                                }
                            }
                            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                return Err(storage_channel_closed_error(&hashed_address))
                            }
                        }
                    }
                    // Since we do not store all intermediate nodes in the database, there might
                    // be a possibility of re-adding a non-modified leaf to the hash builder.
                    None => {
                        tracker.inc_missed_leaves();

                        let is_in_targets = targets.contains_key(&hashed_address);
                        let target_slots_count =
                            targets.get(&hashed_address).map(|s| s.len()).unwrap_or(0);

                        // Track duplicate fallbacks
                        let fallback_count = fallback_tracker.entry(hashed_address).or_insert(0);
                        *fallback_count += 1;
                        let is_duplicate = *fallback_count > 1;

                        // Log trie prefix to see if siblings cluster by prefix
                        let nibbles = Nibbles::unpack(hashed_address);
                        let prefix_hex = format!(
                            "0x{}",
                            nibbles.iter().take(6).map(|n| format!("{:x}", n)).collect::<String>()
                        );

                        warn!(
                            target: "trie::proof",
                            ?hashed_address,
                            prefix_hex,
                            is_in_targets,
                            target_slots_count,
                            fallback_count = *fallback_count,
                            is_duplicate,
                            total_receivers_initially = initial_storage_receivers,
                            "FALLBACK: No receiver - {}",
                            if is_in_targets {
                                "recomputing targeted storage proof"
                            } else {
                                "computing sibling storage proof"
                            }
                        );

                        if is_duplicate {
                            warn!(
                                target: "trie::proof",
                                ?hashed_address,
                                fallback_count = *fallback_count,
                                "DUPLICATE FALLBACK! Same address computed {} times.",
                                *fallback_count
                            );
                        }

                        let (decoded_multiproof, fallback_source) = if is_in_targets {
                            let target_slots =
                                targets.get(&hashed_address).cloned().unwrap_or_default();
                            let raw_fallback_proof = StorageProof::new_hashed(
                                trie_cursor_factory.clone(),
                                hashed_cursor_factory.clone(),
                                hashed_address,
                            )
                            .with_prefix_set_mut(PrefixSetMut::from(
                                target_slots.iter().map(Nibbles::unpack),
                            ))
                            .with_branch_node_masks(collect_branch_node_masks)
                            .with_added_removed_keys(
                                multi_added_removed_keys
                                    .as_ref()
                                    .and_then(|keys| keys.get_storage(&hashed_address)),
                            )
                            .storage_multiproof(target_slots)
                            .map_err(|e| {
                                ParallelStateRootError::StorageRoot(StorageRootError::Database(
                                    DatabaseError::Other(e.to_string()),
                                ))
                            })?;
                            let decoded: DecodedStorageMultiProof =
                                raw_fallback_proof.try_into()?;

                            // Cache the root for potential future reuse
                            missed_leaves_storage_roots.insert(hashed_address, decoded.root);

                            debug!(
                                target: "trie::proof",
                                ?hashed_address,
                                root = ?decoded.root,
                                cache_size = missed_leaves_storage_roots.len(),
                                "Cached storage root from target computation"
                            );

                            (decoded, "target_recompute")
                        } else {
                            match missed_leaves_storage_roots.entry(hashed_address) {
                                Entry::Occupied(occ) => {
                                    tracker.inc_cache_hit();
                                    let root = *occ.get();

                                    info!(
                                        target: "trie::proof",
                                        ?hashed_address,
                                        prefix_hex,
                                        ?root,
                                        cache_size = missed_leaves_storage_roots.len(),
                                        "CACHE HIT: Reusing cached storage root for sibling"
                                    );

                                    let mut proof = DecodedStorageMultiProof::empty();
                                    proof.root = root;
                                    (proof, "cache_hit")
                                }
                                Entry::Vacant(vac) => {
                                    tracker.inc_cache_miss();

                                    debug!(
                                        target: "trie::proof",
                                        ?hashed_address,
                                        prefix_hex,
                                        cache_size = missed_leaves_storage_roots.len(),
                                        "CACHE MISS: Computing storage root for sibling"
                                    );
                                    let raw_fallback_proof = StorageProof::new_hashed(
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
                                            StorageRootError::Database(DatabaseError::Other(
                                                e.to_string(),
                                            )),
                                        )
                                    })?;
                                    let decoded: DecodedStorageMultiProof =
                                        raw_fallback_proof.try_into()?;
                                    let root = decoded.root;
                                    vac.insert(root);

                                    debug!(
                                        target: "trie::proof",
                                        ?hashed_address,
                                        ?root,
                                        cache_size = missed_leaves_storage_roots.len(),
                                        "CACHE INSERT: Stored storage root for sibling"
                                    );

                                    (decoded, "cache_miss_computed")
                                }
                            }
                        };

                        debug!(
                            target: "trie::proof",
                            ?hashed_address,
                            prefix_hex,
                            fallback_source,
                            fallback_count = *fallback_count,
                            "Fallback storage proof resolved"
                        );

                        decoded_multiproof
                    }
                };

                // Encode account
                account_rlp.clear();
                let account = account.into_trie_account(decoded_storage_multiproof.root);
                account.encode(&mut account_rlp as &mut dyn BufMut);

                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);

                // We might be adding leaves that are not necessarily our proof targets.
                if targets.contains_key(&hashed_address) {
                    collected_decoded_storages.insert(hashed_address, decoded_storage_multiproof);
                }
            }
        }
    }

    debug!(
        target: "trie::proof",
        targets_count = targets.len(),
        initial_storage_receivers,
        storage_receivers_remaining = storage_receivers.len(),
        missed_leaves = tracker.missed_leaves(),
        fallback_addresses = fallback_tracker.len(),
        "build_account_multiproof_with_storage finished"
    );
    let _ = hash_builder.root();

    let stats = tracker.finish();

    // Log cache effectiveness
    if stats.missed_leaves() > 0 {
        let hit_rate = stats.cache_hit_rate();
        let cache_saved_computations = stats.cache_hits();

        info!(
            target: "trie::proof",
            missed_leaves = stats.missed_leaves(),
            cache_hits = stats.cache_hits(),
            cache_misses = stats.cache_misses(),
            hit_rate = format!("{:.1}%", hit_rate),
            cache_size = missed_leaves_storage_roots.len(),
            "Cache effectiveness summary"
        );

        if cache_saved_computations > 0 {
            info!(
                target: "trie::proof",
                saved_computations = cache_saved_computations,
                "Cache prevented {} duplicate storage trie walk(s)",
                cache_saved_computations
            );
        }
    }

    let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
    let decoded_account_subtree = DecodedProofNodes::try_from(account_subtree_raw_nodes)?;

    let (branch_node_hash_masks, branch_node_tree_masks) = if collect_branch_node_masks {
        let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
        (
            updated_branch_nodes.iter().map(|(path, node)| (*path, node.hash_mask)).collect(),
            updated_branch_nodes.into_iter().map(|(path, node)| (path, node.tree_mask)).collect(),
        )
    } else {
        (HashMap::default(), HashMap::default())
    };

    trace!(
        target: "trie::parallel_proof",
        duration = ?stats.duration(),
        branches_added = stats.branches_added(),
        leaves_added = stats.leaves_added(),
        missed_leaves = stats.missed_leaves(),
        cache_hits = stats.cache_hits(),
        cache_misses = stats.cache_misses(),
        "Calculated decoded proof"
    );

    Ok((
        DecodedMultiProof {
            account_subtree: decoded_account_subtree,
            branch_node_hash_masks,
            branch_node_tree_masks,
            storages: collected_decoded_storages,
        },
        stats,
    ))
}

/// Parallel proof calculator.
///
/// This can collect proof for many targets in parallel, spawning a task for each hashed address
/// that has proof targets.
#[derive(Debug)]
pub struct ParallelProof<Factory: DatabaseProviderFactory> {
    /// Consistent view of the database.
    view: ConsistentDbView<Factory>,
    /// The sorted collection of cached in-memory intermediate trie nodes that
    /// can be reused for computation.
    pub nodes_sorted: Arc<TrieUpdatesSorted>,
    /// The sorted in-memory overlay hashed state.
    pub state_sorted: Arc<HashedPostStateSorted>,
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `state_sorted` might be present here,
    /// if we have cached nodes for them.
    pub prefix_sets: Arc<TriePrefixSetsMut>,
    /// Flag indicating whether to include branch node masks in the proof.
    collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Handle to the storage proof task.
    storage_proof_task_handle: ProofTaskManagerHandle<FactoryTx<Factory>>,
    /// Cached storage proof roots for missed leaves; this maps
    /// hashed (missed) addresses to their storage proof roots.
    missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
    #[cfg(feature = "metrics")]
    metrics: ParallelTrieMetrics,
}

impl<Factory: DatabaseProviderFactory> ParallelProof<Factory> {
    /// Create new state proof generator.
    pub fn new(
        view: ConsistentDbView<Factory>,
        nodes_sorted: Arc<TrieUpdatesSorted>,
        state_sorted: Arc<HashedPostStateSorted>,
        prefix_sets: Arc<TriePrefixSetsMut>,
        missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
        storage_proof_task_handle: ProofTaskManagerHandle<FactoryTx<Factory>>,
    ) -> Self {
        Self {
            view,
            nodes_sorted,
            state_sorted,
            prefix_sets,
            missed_leaves_storage_roots,
            collect_branch_node_masks: false,
            multi_added_removed_keys: None,
            storage_proof_task_handle,
            #[cfg(feature = "metrics")]
            metrics: ParallelTrieMetrics::new_with_labels(&[("type", "proof")]),
        }
    }

    /// Set the flag indicating whether to include branch node masks in the proof.
    pub const fn with_branch_node_masks(mut self, branch_node_masks: bool) -> Self {
        self.collect_branch_node_masks = branch_node_masks;
        self
    }

    /// Configure the `ParallelProof` with a [`MultiAddedRemovedKeys`], allowing for retaining
    /// extra proofs needed to add and remove leaf nodes from the tries.
    pub fn with_multi_added_removed_keys(
        mut self,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    ) -> Self {
        self.multi_added_removed_keys = multi_added_removed_keys;
        self
    }
}

impl<Factory> ParallelProof<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static,
{
    /// Queues a storage proof task and returns a receiver for the result.
    fn queue_storage_proof(
        &self,
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: B256Set,
    ) -> Receiver<Result<DecodedStorageMultiProof, ParallelStateRootError>> {
        let input = StorageProofInput::new(
            hashed_address,
            prefix_set,
            Arc::new(target_slots),
            self.collect_branch_node_masks,
            self.multi_added_removed_keys.clone(),
        );

        let (sender, receiver) = crossbeam_channel::unbounded();
        if self
            .storage_proof_task_handle
            .queue_task(ProofTaskKind::StorageProof(input, sender))
            .is_err()
        {
            debug!(
                target: "trie::parallel_proof",
                ?hashed_address,
                "Storage proof task queue failed - manager closed"
            );
        }
        receiver
    }

    /// Generate a storage multiproof according to the specified targets and hashed address.
    pub fn storage_proof(
        self,
        hashed_address: B256,
        target_slots: B256Set,
    ) -> Result<DecodedStorageMultiProof, ParallelStateRootError> {
        let total_targets = target_slots.len();
        let prefix_set = PrefixSetMut::from(target_slots.iter().map(Nibbles::unpack));
        let prefix_set = prefix_set.freeze();

        trace!(
            target: "trie::parallel_proof",
            total_targets,
            ?hashed_address,
            "Starting storage proof generation"
        );

        let receiver = self.queue_storage_proof(hashed_address, prefix_set, target_slots);
        let proof_result = receiver.recv().map_err(|_| {
            ParallelStateRootError::StorageRoot(StorageRootError::Database(DatabaseError::Other(
                format!("channel closed for {hashed_address}"),
            )))
        })?;

        trace!(
            target: "trie::parallel_proof",
            total_targets,
            ?hashed_address,
            "Storage proof generation completed"
        );

        proof_result
    }

    /// Generate a state multiproof according to specified targets.
    pub fn decoded_multiproof(
        self,
        targets: MultiProofTargets,
    ) -> Result<DecodedMultiProof, ParallelStateRootError> {
        debug!(
            target: "trie::parallel_proof",
            targets_count = targets.len(),
            "[OLD_PATH] ParallelProof::decoded_multiproof called"
        );

        // Extend prefix sets with targets
        let mut prefix_sets = (*self.prefix_sets).clone();
        prefix_sets.extend(TriePrefixSetsMut {
            account_prefix_set: PrefixSetMut::from(targets.keys().copied().map(Nibbles::unpack)),
            storage_prefix_sets: targets
                .iter()
                .filter(|&(_hashed_address, slots)| !slots.is_empty())
                .map(|(hashed_address, slots)| {
                    (*hashed_address, PrefixSetMut::from(slots.iter().map(Nibbles::unpack)))
                })
                .collect(),
            destroyed_accounts: Default::default(),
        });

        // Freeze prefix sets for storage root targets
        let frozen_prefix_sets = prefix_sets.freeze();

        let storage_root_targets = StorageRootTargets::new(
            frozen_prefix_sets
                .account_prefix_set
                .iter()
                .map(|nibbles| B256::from_slice(&nibbles.pack())),
            frozen_prefix_sets.storage_prefix_sets.clone(),
        );
        let storage_root_targets_len = storage_root_targets.len();

        debug!(
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            account_prefix_set_len = frozen_prefix_sets.account_prefix_set.len(),
            storage_prefix_sets_len = frozen_prefix_sets.storage_prefix_sets.len(),
            "[OLD_PATH] Created storage root targets"
        );

        // stores the receiver for the storage proof outcome for the hashed addresses
        // this way we can lazily await the outcome when we iterate over the map
        let mut storage_proof_receivers: B256Map<
            crossbeam_channel::Receiver<Result<DecodedStorageMultiProof, ParallelStateRootError>>,
        > = B256Map::with_capacity_and_hasher(storage_root_targets.len(), Default::default());

        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let target_slots = targets.get(&hashed_address).cloned().unwrap_or_default();
            let receiver = self.queue_storage_proof(hashed_address, prefix_set, target_slots);

            // store the receiver for that result with the hashed address so we can await this in
            // place when we iterate over the trie
            storage_proof_receivers.insert(hashed_address, receiver);
        }

        debug!(
            target: "trie::parallel_proof",
            receivers_queued = storage_proof_receivers.len(),
            "[OLD_PATH] Queued all storage proof receivers"
        );

        let provider_ro = self.view.provider_ro()?;
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &self.nodes_sorted,
        );
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &self.state_sorted,
        );

        // Build account multiproof with on-demand storage proof fetching
        let (multiproof, stats) = build_account_multiproof_with_storage(
            trie_cursor_factory,
            hashed_cursor_factory,
            targets,
            frozen_prefix_sets,
            storage_proof_receivers, // ← Pass receivers directly for on-demand fetching
            self.collect_branch_node_masks,
            self.multi_added_removed_keys,
            self.missed_leaves_storage_roots.clone(),
        )?;

        #[cfg(feature = "metrics")]
        self.metrics.record(stats);

        Ok(multiproof)
    }

    /// Generate a state multiproof according to specified targets, returning stats.
    ///
    /// This is identical to [`Self::decoded_multiproof`] but also returns the trie statistics
    /// for analysis and testing.
    pub fn decoded_multiproof_with_stats(
        self,
        targets: MultiProofTargets,
    ) -> Result<(DecodedMultiProof, ParallelTrieStats), ParallelStateRootError> {
        // Extend prefix sets with targets
        let mut prefix_sets = (*self.prefix_sets).clone();
        prefix_sets.extend(TriePrefixSetsMut {
            account_prefix_set: PrefixSetMut::from(targets.keys().copied().map(Nibbles::unpack)),
            storage_prefix_sets: targets
                .iter()
                .filter(|&(_hashed_address, slots)| !slots.is_empty())
                .map(|(hashed_address, slots)| {
                    (*hashed_address, PrefixSetMut::from(slots.iter().map(Nibbles::unpack)))
                })
                .collect(),
            destroyed_accounts: Default::default(),
        });

        // Freeze prefix sets for storage root targets
        let frozen_prefix_sets = prefix_sets.freeze();

        let storage_root_targets = StorageRootTargets::new(
            frozen_prefix_sets
                .account_prefix_set
                .iter()
                .map(|nibbles| B256::from_slice(&nibbles.pack())),
            frozen_prefix_sets.storage_prefix_sets.clone(),
        );
        let storage_root_targets_len = storage_root_targets.len();

        debug!(
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            account_prefix_set_len = frozen_prefix_sets.account_prefix_set.len(),
            storage_prefix_sets_len = frozen_prefix_sets.storage_prefix_sets.len(),
            "[OLD_PATH] Created storage root targets"
        );

        // stores the receiver for the storage proof outcome for the hashed addresses
        // this way we can lazily await the outcome when we iterate over the map
        let mut storage_proof_receivers: B256Map<
            crossbeam_channel::Receiver<Result<DecodedStorageMultiProof, ParallelStateRootError>>,
        > = B256Map::with_capacity_and_hasher(storage_root_targets.len(), Default::default());

        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let target_slots = targets.get(&hashed_address).cloned().unwrap_or_default();
            let receiver = self.queue_storage_proof(hashed_address, prefix_set, target_slots);

            // store the receiver for that result with the hashed address so we can await this in
            // place when we iterate over the trie
            storage_proof_receivers.insert(hashed_address, receiver);
        }

        debug!(
            target: "trie::parallel_proof",
            receivers_queued = storage_proof_receivers.len(),
            "[OLD_PATH] Queued all storage proof receivers"
        );

        let provider_ro = self.view.provider_ro()?;
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &self.nodes_sorted,
        );
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &self.state_sorted,
        );

        // Build account multiproof with on-demand storage proof fetching
        let (multiproof, stats) = build_account_multiproof_with_storage(
            trie_cursor_factory,
            hashed_cursor_factory,
            targets,
            frozen_prefix_sets,
            storage_proof_receivers, // ← Pass receivers directly for on-demand fetching
            self.collect_branch_node_masks,
            self.multi_added_removed_keys,
            self.missed_leaves_storage_roots.clone(),
        )?;

        #[cfg(feature = "metrics")]
        self.metrics.record(stats);

        Ok((multiproof, stats))
    }
}

/// Error when storage proof channel closes unexpectedly.
#[inline]
fn storage_channel_closed_error(address: &B256) -> ParallelStateRootError {
    ParallelStateRootError::StorageRoot(StorageRootError::Database(DatabaseError::Other(format!(
        "storage proof channel closed unexpectedly for {address}"
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof_task::{spawn_proof_workers, ProofTaskCtx};
    use alloy_primitives::{
        keccak256,
        map::{B256Set, DefaultHashBuilder},
        Address, U256,
    };
    use rand::Rng;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::proof::Proof;
    use tokio::runtime::Runtime;

    #[test]
    fn random_parallel_proof() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::rng();
        let state = (0..100)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.random::<u64>()), ..Default::default() };
                let mut storage = HashMap::<B256, U256, DefaultHashBuilder>::default();
                let has_storage = rng.random_bool(0.7);
                if has_storage {
                    for _ in 0..100 {
                        storage.insert(
                            B256::from(U256::from(rng.random::<u64>())),
                            U256::from(rng.random::<u64>()),
                        );
                    }
                }
                (address, (account, storage))
            })
            .collect::<HashMap<_, _, DefaultHashBuilder>>();

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw
                .insert_account_for_hashing(
                    state.iter().map(|(address, (account, _))| (*address, Some(*account))),
                )
                .unwrap();
            provider_rw
                .insert_storage_for_hashing(state.iter().map(|(address, (_, storage))| {
                    (
                        *address,
                        storage
                            .iter()
                            .map(|(slot, value)| StorageEntry { key: *slot, value: *value }),
                    )
                }))
                .unwrap();
            provider_rw.commit().unwrap();
        }

        let mut targets = MultiProofTargets::default();
        for (address, (_, storage)) in state.iter().take(10) {
            let hashed_address = keccak256(*address);
            let mut target_slots = B256Set::default();

            for (slot, _) in storage.iter().take(5) {
                target_slots.insert(*slot);
            }

            if !target_slots.is_empty() {
                targets.insert(hashed_address, target_slots);
            }
        }

        let provider_rw = factory.provider_rw().unwrap();
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(provider_rw.tx_ref());
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(provider_rw.tx_ref());

        let rt = Runtime::new().unwrap();

        let task_ctx = ProofTaskCtx::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Arc::new(DashMap::default()),
        );
        let proof_task_handle = spawn_proof_workers(
            rt.handle().clone(),
            consistent_view.clone(),
            task_ctx,
            1, // storage_worker_count
            1, // account_worker_count
            1, // max_concurrency
        )
        .unwrap();

        let parallel_result = ParallelProof::new(
            consistent_view,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            proof_task_handle.clone(),
        )
        .decoded_multiproof(targets.clone())
        .unwrap();

        let sequential_result_raw = Proof::new(trie_cursor_factory, hashed_cursor_factory)
            .multiproof(targets.clone())
            .unwrap(); // targets might be consumed by parallel_result
        let sequential_result_decoded: DecodedMultiProof = sequential_result_raw
            .try_into()
            .expect("Failed to decode sequential_result for test comparison");

        // to help narrow down what is wrong - first compare account subtries
        assert_eq!(parallel_result.account_subtree, sequential_result_decoded.account_subtree);

        // then compare length of all storage subtries
        assert_eq!(parallel_result.storages.len(), sequential_result_decoded.storages.len());

        // then compare each storage subtrie
        for (hashed_address, storage_proof) in &parallel_result.storages {
            let sequential_storage_proof =
                sequential_result_decoded.storages.get(hashed_address).unwrap();
            assert_eq!(storage_proof, sequential_storage_proof);
        }

        // then compare the entire thing for any mask differences
        assert_eq!(parallel_result, sequential_result_decoded);

        // drop the handle to terminate workers
        drop(proof_task_handle);
    }

    /// Test parallel proof with mixed storage targets (some accounts have storage, some don't)
    #[test]
    fn parallel_proof_handles_mixed_storage_targets() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::rng();
        let state = (0..20)
            .map(|i| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.random::<u64>()), ..Default::default() };

                // Every other account has storage
                let mut storage = HashMap::<B256, U256, DefaultHashBuilder>::default();
                if i % 2 == 0 {
                    for _ in 0..10 {
                        storage.insert(
                            B256::from(U256::from(rng.random::<u64>())),
                            U256::from(rng.random::<u64>()),
                        );
                    }
                }
                (address, (account, storage))
            })
            .collect::<HashMap<_, _, DefaultHashBuilder>>();

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw
                .insert_account_for_hashing(
                    state.iter().map(|(address, (account, _))| (*address, Some(*account))),
                )
                .unwrap();
            provider_rw
                .insert_storage_for_hashing(state.iter().map(|(address, (_, storage))| {
                    (
                        *address,
                        storage
                            .iter()
                            .map(|(slot, value)| StorageEntry { key: *slot, value: *value }),
                    )
                }))
                .unwrap();
            provider_rw.commit().unwrap();
        }

        // Create targets with mixed storage (some empty, some with slots)
        let mut targets = MultiProofTargets::default();
        for (address, (_, storage)) in &state {
            let hashed_address = keccak256(*address);
            let target_slots = if storage.is_empty() {
                B256Set::default() // Empty storage
            } else {
                storage.iter().take(3).map(|(slot, _)| *slot).collect()
            };
            targets.insert(hashed_address, target_slots);
        }

        let provider_rw = factory.provider_rw().unwrap();
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(provider_rw.tx_ref());
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(provider_rw.tx_ref());

        let rt = Runtime::new().unwrap();
        let task_ctx = ProofTaskCtx::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Arc::new(DashMap::default()),
        );
        let proof_task_handle = spawn_proof_workers(
            rt.handle().clone(),
            consistent_view.clone(),
            task_ctx,
            2, // storage_worker_count
            1, // account_worker_count
            1, // max_concurrency
        )
        .unwrap();

        let parallel_result = ParallelProof::new(
            consistent_view,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            proof_task_handle.clone(),
        )
        .decoded_multiproof(targets.clone())
        .unwrap();

        let sequential_result_raw =
            Proof::new(trie_cursor_factory, hashed_cursor_factory).multiproof(targets).unwrap();
        let sequential_result_decoded: DecodedMultiProof =
            sequential_result_raw.try_into().unwrap();

        assert_eq!(parallel_result, sequential_result_decoded);

        drop(proof_task_handle);
    }

    /// Test parallel proof with varying storage sizes (validates ordering independence)
    #[test]
    fn parallel_proof_ordering_independence() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::rng();
        // Create state with varying storage sizes to ensure random completion order
        let state = (0..15)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.random::<u64>()), ..Default::default() };

                // Random storage sizes (1-50 slots) to create different proof computation times
                let storage_size = rng.random_range(1..50);
                let storage: HashMap<B256, U256, DefaultHashBuilder> = (0..storage_size)
                    .map(|_| {
                        (
                            B256::from(U256::from(rng.random::<u64>())),
                            U256::from(rng.random::<u64>()),
                        )
                    })
                    .collect();

                (address, (account, storage))
            })
            .collect::<HashMap<_, _, DefaultHashBuilder>>();

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw
                .insert_account_for_hashing(
                    state.iter().map(|(address, (account, _))| (*address, Some(*account))),
                )
                .unwrap();
            provider_rw
                .insert_storage_for_hashing(state.iter().map(|(address, (_, storage))| {
                    (
                        *address,
                        storage
                            .iter()
                            .map(|(slot, value)| StorageEntry { key: *slot, value: *value }),
                    )
                }))
                .unwrap();
            provider_rw.commit().unwrap();
        }

        let mut targets = MultiProofTargets::default();
        for (address, (_, storage)) in &state {
            let hashed_address = keccak256(*address);
            let target_slots: B256Set = storage.keys().take(5).copied().collect();
            if !target_slots.is_empty() {
                targets.insert(hashed_address, target_slots);
            }
        }

        let provider_rw = factory.provider_rw().unwrap();
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(provider_rw.tx_ref());
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(provider_rw.tx_ref());

        let rt = Runtime::new().unwrap();
        let task_ctx = ProofTaskCtx::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Arc::new(DashMap::default()),
        );

        // Use 3 workers to increase chance of out-of-order completion
        let proof_task_handle = spawn_proof_workers(
            rt.handle().clone(),
            consistent_view.clone(),
            task_ctx,
            3, // storage_worker_count
            1, // account_worker_count
            1, // max_concurrency
        )
        .unwrap();

        let parallel_result = ParallelProof::new(
            consistent_view,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            proof_task_handle.clone(),
        )
        .decoded_multiproof(targets.clone())
        .unwrap();

        let sequential_result_raw =
            Proof::new(trie_cursor_factory, hashed_cursor_factory).multiproof(targets).unwrap();
        let sequential_result_decoded: DecodedMultiProof =
            sequential_result_raw.try_into().unwrap();

        // Results should be identical regardless of completion order
        assert_eq!(parallel_result, sequential_result_decoded);

        drop(proof_task_handle);
    }

    /// Test that storage proof metrics are properly tracked
    #[test]
    fn storage_proof_metrics_tracking() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::rng();
        // Create state with multiple accounts having storage
        let state = (0..10)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.random::<u64>()), ..Default::default() };

                // All accounts have storage to ensure we get metrics
                let storage: HashMap<B256, U256, DefaultHashBuilder> = (0..5)
                    .map(|_| {
                        (
                            B256::from(U256::from(rng.random::<u64>())),
                            U256::from(rng.random::<u64>()),
                        )
                    })
                    .collect();
                (address, (account, storage))
            })
            .collect::<HashMap<_, _, DefaultHashBuilder>>();

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw
                .insert_account_for_hashing(
                    state.iter().map(|(address, (account, _))| (*address, Some(*account))),
                )
                .unwrap();
            provider_rw
                .insert_storage_for_hashing(state.iter().map(|(address, (_, storage))| {
                    (
                        *address,
                        storage
                            .iter()
                            .map(|(slot, value)| StorageEntry { key: *slot, value: *value }),
                    )
                }))
                .unwrap();
            provider_rw.commit().unwrap();
        }

        // Create targets for all accounts
        let mut targets = MultiProofTargets::default();
        for (address, (_, storage)) in &state {
            let hashed_address = keccak256(*address);
            let target_slots = storage.iter().take(3).map(|(slot, _)| *slot).collect();
            targets.insert(hashed_address, target_slots);
        }

        let rt = Runtime::new().unwrap();
        let task_ctx = ProofTaskCtx::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Arc::new(DashMap::default()),
        );
        let proof_task_handle = spawn_proof_workers(
            rt.handle().clone(),
            consistent_view.clone(),
            task_ctx,
            2, // storage_worker_count
            1, // account_worker_count
            1, // max_concurrency
        )
        .unwrap();

        let (result, stats) = ParallelProof::new(
            consistent_view,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            proof_task_handle.clone(),
        )
        .decoded_multiproof_with_stats(targets.clone())
        .unwrap();

        // Verify we got a valid result
        assert!(!result.account_subtree.is_empty());

        // Verify metrics are tracked
        let total_proofs = stats.storage_proofs_immediate() + stats.storage_proofs_blocked();
        assert_eq!(total_proofs, 10, "Should track all 10 storage proofs (immediate + blocked)");

        // At least one category should have proofs
        assert!(
            stats.storage_proofs_immediate() > 0 || stats.storage_proofs_blocked() > 0,
            "Should have at least some storage proof activity"
        );

        // Percentage should be valid (0-100)
        let percentage = stats.storage_proofs_immediate_percentage();
        assert!(
            (0.0..=100.0).contains(&percentage),
            "Percentage should be between 0-100, got {percentage}"
        );

        drop(proof_task_handle);
    }
}
