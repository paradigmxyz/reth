use crate::{
    metrics::ParallelTrieMetrics,
    proof_task::{ProofTaskMessage, StorageProofInput},
    root::ParallelStateRootError,
    stats::ParallelTrieTracker,
    StorageRootTargets,
};
use alloy_primitives::{
    map::{B256Map, HashMap},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, FactoryTx,
    ProviderError, StateCommitmentProvider,
};
use reth_storage_errors::db::DatabaseError;
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
use std::sync::{mpsc::Sender, Arc};
use tracing::debug;

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
    /// Sender to the storage proof task.
    storage_proof_task: Sender<ProofTaskMessage<FactoryTx<Factory>>>,
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
        storage_proof_task: Sender<ProofTaskMessage<FactoryTx<Factory>>>,
    ) -> Self {
        Self {
            view,
            nodes_sorted,
            state_sorted,
            prefix_sets,
            collect_branch_node_masks: false,
            storage_proof_task,
            #[cfg(feature = "metrics")]
            metrics: ParallelTrieMetrics::new_with_labels(&[("type", "proof")]),
        }
    }

    /// Set the flag indicating whether to include branch node masks in the proof.
    pub const fn with_branch_node_masks(mut self, branch_node_masks: bool) -> Self {
        self.collect_branch_node_masks = branch_node_masks;
        self
    }
}

impl<Factory> ParallelProof<Factory>
where
    Factory:
        DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + Clone + 'static,
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
                .filter(|&(_hashed_address, slots)| !slots.is_empty())
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
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            "Starting parallel proof generation"
        );

        // Pre-calculate storage roots for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

        // stores the receiver for the storage proof outcome for the hashed addresses
        // this way we can lazily await the outcome when we iterate over the map
        let mut storage_proofs =
            B256Map::with_capacity_and_hasher(storage_root_targets.len(), Default::default());

        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let target_slots = targets.get(&hashed_address).cloned().unwrap_or_default();

            let input = StorageProofInput::new(
                hashed_address,
                prefix_set,
                target_slots,
                self.collect_branch_node_masks,
            );
            let (sender, receiver) = std::sync::mpsc::channel();
            let _ = self.storage_proof_task.send(ProofTaskMessage::StorageProof((input, sender)));

            // store the receiver for that result with the hashed address so we can await this in
            // place when we iterate over the trie
            storage_proofs.insert(hashed_address, receiver);
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
            .with_updates(self.collect_branch_node_masks);

        // Initialize all storage multiproofs as empty.
        // Storage multiproofs for non empty tries will be overwritten if necessary.
        let mut storages: B256Map<_> =
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

        let stats = tracker.finish();
        #[cfg(feature = "metrics")]
        self.metrics.record(stats);

        let account_subtree = hash_builder.take_proof_nodes();
        let (branch_node_hash_masks, branch_node_tree_masks) = if self.collect_branch_node_masks {
            let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
            (
                updated_branch_nodes
                    .iter()
                    .map(|(path, node)| (path.clone(), node.hash_mask))
                    .collect(),
                updated_branch_nodes
                    .into_iter()
                    .map(|(path, node)| (path, node.tree_mask))
                    .collect(),
            )
        } else {
            (HashMap::default(), HashMap::default())
        };

        debug!(
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            duration = ?stats.duration(),
            branches_added = stats.branches_added(),
            leaves_added = stats.leaves_added(),
            missed_leaves = stats.missed_leaves(),
            precomputed_storage_roots = stats.precomputed_storage_roots(),
            "Calculated proof"
        );

        Ok(MultiProof { account_subtree, branch_node_hash_masks, branch_node_tree_masks, storages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof_task::{ProofTaskCtx, ProofTaskManager};
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

        let task_ctx = ProofTaskCtx::new(Default::default(), Default::default());
        let proof_task =
            ProofTaskManager::new(rt.handle().clone(), consistent_view.clone(), task_ctx, 1);
        let proof_task_handle = proof_task.handle();
        let proof_task_sender = proof_task_handle.sender();

        // keep the join handle around to make sure it does not return any errors
        // after we compute the state root
        let join_handle = rt.spawn_blocking(move || proof_task.run());

        let parallel_result = ParallelProof::new(
            consistent_view,
            Default::default(),
            Default::default(),
            Default::default(),
            proof_task_sender,
        )
        .multiproof(targets.clone())
        .unwrap();

        let sequential_result =
            Proof::new(trie_cursor_factory, hashed_cursor_factory).multiproof(targets).unwrap();

        // to help narrow down what is wrong - first compare account subtries
        assert_eq!(parallel_result.account_subtree, sequential_result.account_subtree);

        // then compare length of all storage subtries
        assert_eq!(parallel_result.storages.len(), sequential_result.storages.len());

        // then compare each storage subtrie
        for (hashed_address, storage_proof) in &parallel_result.storages {
            let sequential_storage_proof = sequential_result.storages.get(hashed_address).unwrap();
            assert_eq!(storage_proof, sequential_storage_proof);
        }

        // then compare the entire thing for any mask differences
        assert_eq!(parallel_result, sequential_result);

        // drop the handle to terminate the task and then block on the proof task handle to make
        // sure it does not return any errors
        drop(proof_task_handle);
        rt.block_on(join_handle).unwrap().expect("The proof task should not return an error");
    }
}
