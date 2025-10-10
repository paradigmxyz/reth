use crate::{
    metrics::ParallelTrieMetrics,
    proof_task::{
        AccountMultiproofInput, ProofTaskKind, ProofTaskManagerHandle, StorageProofInput,
    },
    root::ParallelStateRootError,
    StorageRootTargets,
};
use alloy_primitives::{map::B256Set, B256};
use dashmap::DashMap;
use reth_execution_errors::StorageRootError;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSets, TriePrefixSetsMut},
    updates::TrieUpdatesSorted,
    DecodedMultiProof, DecodedStorageMultiProof, HashedPostStateSorted, MultiProofTargets, Nibbles,
};
use reth_trie_common::added_removed_keys::MultiAddedRemovedKeys;
use std::sync::{
    mpsc::{channel, Receiver},
    Arc,
};
use tracing::trace;

/// Parallel proof calculator.
///
/// This can collect proof for many targets in parallel, spawning a task for each hashed address
/// that has proof targets.
#[derive(Debug)]
pub struct ParallelProof {
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
    /// Handle to the proof task manager.
    proof_task_handle: ProofTaskManagerHandle,
    /// Cached storage proof roots for missed leaves; this maps
    /// hashed (missed) addresses to their storage proof roots.
    missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
    #[cfg(feature = "metrics")]
    metrics: ParallelTrieMetrics,
}

impl ParallelProof {
    /// Create new state proof generator.
    pub fn new(
        nodes_sorted: Arc<TrieUpdatesSorted>,
        state_sorted: Arc<HashedPostStateSorted>,
        prefix_sets: Arc<TriePrefixSetsMut>,
        missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
        proof_task_handle: ProofTaskManagerHandle,
    ) -> Self {
        Self {
            nodes_sorted,
            state_sorted,
            prefix_sets,
            missed_leaves_storage_roots,
            collect_branch_node_masks: false,
            multi_added_removed_keys: None,
            proof_task_handle,
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
            target_slots,
            self.collect_branch_node_masks,
            self.multi_added_removed_keys.clone(),
        );

        let (sender, receiver) = std::sync::mpsc::channel();
        let _ = self.proof_task_handle.queue_task(ProofTaskKind::StorageProof(input, sender));
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

    /// Extends prefix sets with the given multiproof targets and returns the frozen result.
    ///
    /// This is a helper function used to prepare prefix sets before computing multiproofs.
    /// Returns frozen (immutable) prefix sets ready for use in proof computation.
    pub fn extend_prefix_sets_with_targets(
        base_prefix_sets: &TriePrefixSetsMut,
        targets: &MultiProofTargets,
    ) -> TriePrefixSets {
        let mut extended = base_prefix_sets.clone();
        extended.extend(TriePrefixSetsMut {
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
        extended.freeze()
    }

    /// Generate a state multiproof according to specified targets.
    pub fn decoded_multiproof(
        self,
        targets: MultiProofTargets,
    ) -> Result<DecodedMultiProof, ParallelStateRootError> {
        // Extend prefix sets with targets
        let prefix_sets = Self::extend_prefix_sets_with_targets(&self.prefix_sets, &targets);

        let storage_root_targets_len = StorageRootTargets::count(
            &prefix_sets.account_prefix_set,
            &prefix_sets.storage_prefix_sets,
        );

        trace!(
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            "Starting parallel proof generation"
        );

        // Queue account multiproof request to account worker pool

        let input = AccountMultiproofInput {
            targets,
            prefix_sets,
            collect_branch_node_masks: self.collect_branch_node_masks,
            multi_added_removed_keys: self.multi_added_removed_keys.clone(),
            missed_leaves_storage_roots: self.missed_leaves_storage_roots.clone(),
        };

        let (sender, receiver) = channel();
        self.proof_task_handle
            .queue_task(ProofTaskKind::AccountMultiproof(input, sender))
            .map_err(|_| {
                ParallelStateRootError::Other(
                    "Failed to queue account multiproof: account worker pool unavailable"
                        .to_string(),
                )
            })?;

        // Wait for account multiproof result from worker
        let (multiproof, stats) = receiver.recv().map_err(|_| {
            ParallelStateRootError::Other(
                "Account multiproof channel dropped: worker died or pool shutdown".to_string(),
            )
        })??;

        #[cfg(feature = "metrics")]
        self.metrics.record(stats);

        trace!(
            target: "trie::parallel_proof",
            total_targets = storage_root_targets_len,
            duration = ?stats.duration(),
            branches_added = stats.branches_added(),
            leaves_added = stats.leaves_added(),
            missed_leaves = stats.missed_leaves(),
            precomputed_storage_roots = stats.precomputed_storage_roots(),
            "Calculated decoded proof"
        );

        Ok(multiproof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof_task::{ProofTaskCtx, ProofTaskManager};
    use alloy_primitives::{
        keccak256,
        map::{B256Set, DefaultHashBuilder, HashMap},
        Address, U256,
    };
    use rand::Rng;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        providers::ConsistentDbView,
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        HashingWriter, ProviderFactory,
    };
    use reth_trie::proof::Proof;
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
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

        let task_ctx =
            ProofTaskCtx::new(Default::default(), Default::default(), Default::default());
        let proof_task =
            ProofTaskManager::new(rt.handle().clone(), consistent_view, task_ctx, 1, 1).unwrap();
        let proof_task_handle = proof_task.handle();

        // keep the join handle around to make sure it does not return any errors
        // after we compute the state root
        let join_handle = rt.spawn_blocking(move || proof_task.run());

        type Factory = ProviderFactory<MockNodeTypesWithDB>;
        let parallel_result = ParallelProof::new(
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

        // drop the handle to terminate the task and then block on the proof task handle to make
        // sure it does not return any errors
        drop(proof_task_handle);
        rt.block_on(join_handle).unwrap().expect("The proof task should not return an error");
    }
}
