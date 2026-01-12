use crate::{
    metrics::ParallelTrieMetrics,
    proof_task::{AccountMultiproofInput, ProofResult, ProofResultContext, ProofWorkerHandle},
    root::ParallelStateRootError,
    StorageRootTargets,
};
use crossbeam_channel::unbounded as crossbeam_unbounded;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets, TriePrefixSetsMut},
    DecodedMultiProof, HashedPostState, MultiProofTargets, Nibbles,
};
use reth_trie_common::added_removed_keys::MultiAddedRemovedKeys;
use std::{sync::Arc, time::Instant};
use tracing::trace;

/// Parallel proof calculator.
///
/// This can collect proof for many targets in parallel, spawning a task for each hashed address
/// that has proof targets.
#[derive(Debug)]
pub struct ParallelProof {
    /// The collection of prefix sets for the computation.
    pub prefix_sets: Arc<TriePrefixSetsMut>,
    /// Flag indicating whether to include branch node masks in the proof.
    collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Handle to the proof worker pools.
    proof_worker_handle: ProofWorkerHandle,
    /// Whether to use V2 storage proofs.
    v2_proofs_enabled: bool,
    #[cfg(feature = "metrics")]
    metrics: ParallelTrieMetrics,
}

impl ParallelProof {
    /// Create new state proof generator.
    pub fn new(
        prefix_sets: Arc<TriePrefixSetsMut>,
        proof_worker_handle: ProofWorkerHandle,
    ) -> Self {
        Self {
            prefix_sets,
            collect_branch_node_masks: false,
            multi_added_removed_keys: None,
            proof_worker_handle,
            v2_proofs_enabled: false,
            #[cfg(feature = "metrics")]
            metrics: ParallelTrieMetrics::new_with_labels(&[("type", "proof")]),
        }
    }

    /// Set whether to use V2 storage proofs.
    pub const fn with_v2_proofs_enabled(mut self, v2_proofs_enabled: bool) -> Self {
        self.v2_proofs_enabled = v2_proofs_enabled;
        self
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
        // Create channel for receiving ProofResultMessage
        let (result_tx, result_rx) = crossbeam_unbounded();
        let account_multiproof_start_time = Instant::now();

        let input = AccountMultiproofInput {
            targets,
            prefix_sets,
            collect_branch_node_masks: self.collect_branch_node_masks,
            multi_added_removed_keys: self.multi_added_removed_keys.clone(),
            proof_result_sender: ProofResultContext::new(
                result_tx,
                0,
                HashedPostState::default(),
                account_multiproof_start_time,
            ),
            v2_proofs_enabled: self.v2_proofs_enabled,
        };

        self.proof_worker_handle
            .dispatch_account_multiproof(input)
            .map_err(|e| ParallelStateRootError::Other(e.to_string()))?;

        // Wait for account multiproof result from worker
        let proof_result_msg = result_rx.recv().map_err(|_| {
            ParallelStateRootError::Other(
                "Account multiproof channel dropped: worker died or pool shutdown".to_string(),
            )
        })?;

        let ProofResult { proof: multiproof, stats } = proof_result_msg.result?;

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
    use crate::proof_task::{ProofTaskCtx, ProofWorkerHandle};
    use alloy_primitives::{
        keccak256,
        map::{B256Set, DefaultHashBuilder, HashMap},
        Address, B256, U256,
    };
    use rand::Rng;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::proof::Proof;
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
    use tokio::runtime::Runtime;

    #[test]
    fn random_parallel_proof() {
        let factory = create_test_provider_factory();

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

        let factory = reth_provider::providers::OverlayStateProviderFactory::new(factory);
        let task_ctx = ProofTaskCtx::new(factory);
        let proof_worker_handle =
            ProofWorkerHandle::new(rt.handle().clone(), task_ctx, 1, 1, false);

        let parallel_result = ParallelProof::new(Default::default(), proof_worker_handle.clone())
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

        // Workers shut down automatically when handle is dropped
        drop(proof_worker_handle);
    }
}
