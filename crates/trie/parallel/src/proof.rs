use crate::{root::ParallelStateRootError, stats::ParallelTrieTracker, StorageRootTargets};
use alloy_primitives::{
    map::{B256HashMap, HashMap},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_db::DatabaseError;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
    StateCommitmentProvider,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    proof::StorageProof,
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    walker::TrieWalker,
    HashBuilder, HashedPostStateSorted, MultiProof, MultiProofTargets, Nibbles, StorageMultiProof,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::proof::ProofRetainer;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{sync::Arc, time::Instant};
use tracing::{debug, trace};

#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;

/// TODO:
#[derive(Debug)]
pub struct ParallelProof<'env, Factory> {
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
    /// Flag indicating whether to include branch node hash masks in the proof.
    collect_branch_node_hash_masks: bool,
    /// Thread pool for local tasks
    thread_pool: &'env rayon::ThreadPool,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<'env, Factory> ParallelProof<'env, Factory> {
    /// Create new state proof generator.
    pub fn new(
        view: ConsistentDbView<Factory>,
        nodes_sorted: Arc<TrieUpdatesSorted>,
        state_sorted: Arc<HashedPostStateSorted>,
        prefix_sets: Arc<TriePrefixSetsMut>,
        thread_pool: &'env rayon::ThreadPool,
    ) -> Self {
        Self {
            view,
            nodes_sorted,
            state_sorted,
            prefix_sets,
            collect_branch_node_hash_masks: false,
            thread_pool,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }

    /// Set the flag indicating whether to include branch node hash masks in the proof.
    pub const fn with_branch_node_hash_masks(mut self, branch_node_hash_masks: bool) -> Self {
        self.collect_branch_node_hash_masks = branch_node_hash_masks;
        self
    }
}

impl<Factory> ParallelProof<'_, Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader>
        + StateCommitmentProvider
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Generate a state multiproof according to specified targets.
    pub fn multiproof(
        self,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, ParallelStateRootError> {
        let mut tracker = ParallelTrieTracker::default();

        // Extend prefix sets with targets
        let mut prefix_sets = (*self.prefix_sets).clone();
        prefix_sets.extend(TriePrefixSetsMut {
            account_prefix_set: PrefixSetMut::from(targets.keys().copied().map(Nibbles::unpack)),
            storage_prefix_sets: targets
                .iter()
                .filter(|&(_hashed_address, slots)| (!slots.is_empty()))
                .map(|(hashed_address, slots)| {
                    (*hashed_address, PrefixSetMut::from(slots.iter().map(Nibbles::unpack)))
                })
                .collect(),
            destroyed_accounts: Default::default(),
        });
        let prefix_sets = prefix_sets.freeze();

        let storage_root_targets = StorageRootTargets::new(
            prefix_sets.account_prefix_set.iter().map(|nibbles| B256::from_slice(&nibbles.pack())),
            prefix_sets.storage_prefix_sets.clone(),
        );
        let storage_root_targets_len = storage_root_targets.len();

        debug!(
            target: "trie::parallel_state_root",
            total_targets = storage_root_targets_len,
            "Starting parallel proof generation"
        );

        // Pre-calculate storage roots for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

        let mut storage_proofs =
            B256HashMap::with_capacity_and_hasher(storage_root_targets.len(), Default::default());

        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let view = self.view.clone();
            let target_slots = targets.get(&hashed_address).cloned().unwrap_or_default();
            let trie_nodes_sorted = self.nodes_sorted.clone();
            let hashed_state_sorted = self.state_sorted.clone();
            let collect_masks = self.collect_branch_node_hash_masks;

            let (tx, rx) = std::sync::mpsc::sync_channel(1);

            self.thread_pool.spawn_fifo(move || {
                debug!(
                    target: "trie::parallel",
                    ?hashed_address,
                    "Starting proof calculation"
                );

                let task_start = Instant::now();
                let result = (|| -> Result<_, ParallelStateRootError> {
                    let provider_start = Instant::now();
                    let provider_ro = view.provider_ro()?;
                    trace!(
                        target: "trie::parallel",
                        ?hashed_address,
                        provider_time = ?provider_start.elapsed(),
                        "Got provider"
                    );

                    let cursor_start = Instant::now();
                    let trie_cursor_factory = InMemoryTrieCursorFactory::new(
                        DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                        &trie_nodes_sorted,
                    );
                    let hashed_cursor_factory = HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                        &hashed_state_sorted,
                    );
                    trace!(
                        target: "trie::parallel",
                        ?hashed_address,
                        cursor_time = ?cursor_start.elapsed(),
                        "Created cursors"
                    );

                    let proof_start = Instant::now();
                    let proof_result = StorageProof::new_hashed(
                        trie_cursor_factory,
                        hashed_cursor_factory,
                        hashed_address,
                    )
                    .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().cloned()))
                    .with_branch_node_hash_masks(collect_masks)
                    .storage_multiproof(target_slots)
                    .map_err(|e| ParallelStateRootError::Other(e.to_string()));

                    trace!(
                        target: "trie::parallel",
                        ?hashed_address,
                        proof_time = ?proof_start.elapsed(),
                        "Completed proof calculation"
                    );

                    proof_result
                })();

                // We can have the receiver dropped before we send, because we still calculate
                // storage proofs for deleted accounts, but do not actually walk over them in
                // `account_node_iter` below.
                if let Err(e) = tx.send(result) {
                    debug!(
                        target: "trie::parallel",
                        ?hashed_address,
                        error = ?e,
                        task_time = ?task_start.elapsed(),
                        "Failed to send proof result"
                    );
                }
            });
            storage_proofs.insert(hashed_address, rx);
        }

        let provider_ro = self.view.provider_ro()?;
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &self.nodes_sorted,
        );
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &self.state_sorted,
        );

        // Create the walker.
        let walker = TrieWalker::new(
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
            prefix_sets.account_prefix_set,
        )
        .with_deletions_retained(true);

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer: ProofRetainer = targets.keys().map(Nibbles::unpack).collect();
        let mut hash_builder = HashBuilder::default()
            .with_proof_retainer(retainer)
            .with_updates(self.collect_branch_node_hash_masks);

        // Initialize all storage multiproofs as empty.
        // Storage multiproofs for non empty tries will be overwritten if necessary.
        let mut storages: B256HashMap<_> =
            targets.keys().map(|key| (*key, StorageMultiProof::empty())).collect();
        let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
        let mut account_node_iter = TrieNodeIter::new(
            walker,
            hashed_cursor_factory.hashed_account_cursor().map_err(ProviderError::Database)?,
        );
        while let Some(account_node) =
            account_node_iter.try_next().map_err(ProviderError::Database)?
        {
            match account_node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let storage_multiproof = match storage_proofs.remove(&hashed_address) {
                        Some(rx) => rx.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(StorageRootError::Database(
                                DatabaseError::Other(format!(
                                    "channel closed for {hashed_address}"
                                )),
                            ))
                        })??,
                        // Since we do not store all intermediate nodes in the database, there might
                        // be a possibility of re-adding a non-modified leaf to the hash builder.
                        None => {
                            tracker.inc_missed_leaves();
                            StorageProof::new_hashed(
                                trie_cursor_factory.clone(),
                                hashed_cursor_factory.clone(),
                                hashed_address,
                            )
                            .with_prefix_set_mut(Default::default())
                            .storage_multiproof(
                                targets.get(&hashed_address).cloned().unwrap_or_default(),
                            )
                            .map_err(|e| {
                                ParallelStateRootError::StorageRoot(StorageRootError::Database(
                                    DatabaseError::Other(e.to_string()),
                                ))
                            })?
                        }
                    };

                    // Encode account
                    account_rlp.clear();
                    let account = account.into_trie_account(storage_multiproof.root);
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);

                    // We might be adding leaves that are not necessarily our proof targets.
                    if targets.contains_key(&hashed_address) {
                        storages.insert(hashed_address, storage_multiproof);
                    }
                }
            }
        }
        let _ = hash_builder.root();

        #[cfg(feature = "metrics")]
        self.metrics.record_state_trie(tracker.finish());

        let account_subtree = hash_builder.take_proof_nodes();
        let branch_node_hash_masks = if self.collect_branch_node_hash_masks {
            hash_builder
                .updated_branch_nodes
                .unwrap_or_default()
                .into_iter()
                .map(|(path, node)| (path, node.hash_mask))
                .collect()
        } else {
            HashMap::default()
        };

        Ok(MultiProof { account_subtree, branch_node_hash_masks, storages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{
        keccak256,
        map::{B256HashSet, DefaultHashBuilder},
        Address, U256,
    };
    use rand::Rng;
    use reth_primitives::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::proof::Proof;

    #[test]
    fn random_parallel_proof() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::thread_rng();
        let state = (0..100)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.gen::<u64>()), ..Default::default() };
                let mut storage = HashMap::<B256, U256, DefaultHashBuilder>::default();
                let has_storage = rng.gen_bool(0.7);
                if has_storage {
                    for _ in 0..100 {
                        storage.insert(
                            B256::from(U256::from(rng.gen::<u64>())),
                            U256::from(rng.gen::<u64>()),
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
            let mut target_slots = B256HashSet::default();

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

        let num_threads =
            std::thread::available_parallelism().map_or(1, |num| (num.get() / 2).max(1));

        let state_root_task_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|i| format!("proof-worker-{}", i))
            .build()
            .expect("Failed to create proof worker thread pool");

        assert_eq!(
            ParallelProof::new(
                consistent_view,
                Default::default(),
                Default::default(),
                Default::default(),
                &state_root_task_pool
            )
            .multiproof(targets.clone())
            .unwrap(),
            Proof::new(trie_cursor_factory, hashed_cursor_factory).multiproof(targets).unwrap()
        );
    }
}
