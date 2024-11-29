use crate::{root::ParallelStateRootError, stats::ParallelTrieTracker, StorageRootTargets};
use alloy_primitives::{
    map::{HashMap, HashSet},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_db::DatabaseError;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    proof::StorageProof,
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    walker::TrieWalker,
    HashBuilder, MultiProof, Nibbles, TrieAccount, TrieInput, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::proof::ProofRetainer;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::sync::Arc;
use tracing::{debug, error};

#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;

/// TODO:
#[derive(Debug)]
pub struct ParallelProof<Factory> {
    /// Consistent view of the database.
    view: ConsistentDbView<Factory>,
    /// Trie input.
    input: Arc<TrieInput>,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<Factory> ParallelProof<Factory> {
    /// Create new state proof generator.
    pub fn new(view: ConsistentDbView<Factory>, input: Arc<TrieInput>) -> Self {
        Self {
            view,
            input,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }
}

impl<Factory> ParallelProof<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    /// Generate a state multiproof according to specified targets.
    pub fn multiproof(
        self,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> Result<MultiProof, ParallelStateRootError> {
        let mut tracker = ParallelTrieTracker::default();

        let trie_nodes_sorted = self.input.nodes.clone().into_sorted();
        let hashed_state_sorted = self.input.state.clone().into_sorted();

        // Extend prefix sets with targets
        let mut prefix_sets = self.input.prefix_sets.clone();
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

        // Pre-calculate storage roots for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets.len() as u64);
        debug!(target: "trie::parallel_state_root", len = storage_root_targets.len(), "pre-generating storage proofs");
        let mut storage_proofs = HashMap::with_capacity(storage_root_targets.len());
        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let view = self.view.clone();
            let target_slots: HashSet<B256> =
                targets.get(&hashed_address).cloned().unwrap_or_default();

            let trie_nodes_sorted = trie_nodes_sorted.clone();
            let hashed_state_sorted = hashed_state_sorted.clone();

            let (tx, rx) = std::sync::mpsc::sync_channel(1);

            rayon::spawn_fifo(move || {
                let result = (|| -> Result<_, ParallelStateRootError> {
                    let provider_ro = view.provider_ro()?;
                    let trie_cursor_factory = InMemoryTrieCursorFactory::new(
                        DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                        &trie_nodes_sorted,
                    );
                    let hashed_cursor_factory = HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                        &hashed_state_sorted,
                    );

                    StorageProof::new_hashed(
                        trie_cursor_factory,
                        hashed_cursor_factory,
                        hashed_address,
                    )
                    .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().cloned()))
                    .storage_multiproof(target_slots)
                    .map_err(|e| {
                        ParallelStateRootError::StorageRoot(StorageRootError::Database(
                            DatabaseError::Other(e.to_string()),
                        ))
                    })
                })();
                if let Err(err) = tx.send(result) {
                    error!(target: "trie::parallel", ?hashed_address, err_content = ?err.0,  "Failed to send proof result");
                }
            });
            storage_proofs.insert(hashed_address, rx);
        }

        let provider_ro = self.view.provider_ro()?;
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &trie_nodes_sorted,
        );
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &hashed_state_sorted,
        );

        // Create the walker.
        let walker = TrieWalker::new(
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
            prefix_sets.account_prefix_set,
        )
        .with_deletions_retained(true);

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer: ProofRetainer = targets.keys().map(Nibbles::unpack).collect();
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);

        let mut storages = HashMap::default();
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
                    let account = TrieAccount::from((account, storage_multiproof.root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                    storages.insert(hashed_address, storage_multiproof);
                }
            }
        }
        let _ = hash_builder.root();

        #[cfg(feature = "metrics")]
        self.metrics.record_state_trie(tracker.finish());

        Ok(MultiProof { account_subtree: hash_builder.take_proof_nodes(), storages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, map::DefaultHashBuilder, Address, U256};
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

        let mut targets =
            HashMap::<B256, HashSet<B256, DefaultHashBuilder>, DefaultHashBuilder>::default();
        for (address, (_, storage)) in state.iter().take(10) {
            let hashed_address = keccak256(*address);
            let mut target_slots = HashSet::<B256, DefaultHashBuilder>::default();

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

        assert_eq!(
            ParallelProof::new(consistent_view, Default::default())
                .multiproof(targets.clone())
                .unwrap(),
            Proof::new(trie_cursor_factory, hashed_cursor_factory).multiproof(targets).unwrap()
        );
    }
}
