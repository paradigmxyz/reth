#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;
use crate::{stats::ParallelTrieTracker, storage_root_targets::StorageRootTargets};
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdates,
    walker::TrieWalker,
    HashBuilder, Nibbles, StorageRoot, TrieAccount, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{collections::HashMap, sync::Arc};
use thiserror::Error;
use tracing::*;

/// Parallel incremental state root calculator.
///
/// The calculator starts off by launching tasks to compute storage roots.
/// Then, it immediately starts walking the state trie updating the necessary trie
/// nodes in the process. Upon encountering a leaf node, it will poll the storage root
/// task for the corresponding hashed address.
///
/// Internally, the calculator uses [`ConsistentDbView`] since
/// it needs to rely on database state saying the same until
/// the last transaction is open.
/// See docs of using [`ConsistentDbView`] for caveats.
#[derive(Debug)]
pub struct ParallelStateRoot<Factory> {
    /// Consistent view of the database.
    view: ConsistentDbView<Factory>,
    /// Trie input.
    input: TrieInput,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<Factory> ParallelStateRoot<Factory> {
    /// Create new parallel state root calculator.
    pub fn new(view: ConsistentDbView<Factory>, input: TrieInput) -> Self {
        Self {
            view,
            input,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }
}

impl<Factory> ParallelStateRoot<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    /// Calculate incremental state root in parallel.
    pub fn incremental_root(self) -> Result<B256, ParallelStateRootError> {
        self.calculate(false).map(|(root, _)| root)
    }

    /// Calculate incremental state root with updates in parallel.
    pub fn incremental_root_with_updates(
        self,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        self.calculate(true)
    }

    fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let mut tracker = ParallelTrieTracker::default();
        let trie_nodes_sorted = Arc::new(self.input.nodes.into_sorted());
        let hashed_state_sorted = Arc::new(self.input.state.into_sorted());
        let prefix_sets = self.input.prefix_sets.freeze();
        let storage_root_targets = StorageRootTargets::new(
            prefix_sets.account_prefix_set.iter().map(|nibbles| B256::from_slice(&nibbles.pack())),
            prefix_sets.storage_prefix_sets,
        );

        // Pre-calculate storage roots in parallel for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets.len() as u64);
        debug!(target: "trie::parallel_state_root", len = storage_root_targets.len(), "pre-calculating storage roots");
        let mut storage_roots = HashMap::with_capacity(storage_root_targets.len());
        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let view = self.view.clone();
            let hashed_state_sorted = hashed_state_sorted.clone();
            let trie_nodes_sorted = trie_nodes_sorted.clone();
            #[cfg(feature = "metrics")]
            let metrics = self.metrics.storage_trie.clone();

            let (tx, rx) = std::sync::mpsc::sync_channel(1);

            rayon::spawn_fifo(move || {
                let result = (|| -> Result<_, ParallelStateRootError> {
                    let provider_ro = view.provider_ro()?;
                    let trie_cursor_factory = InMemoryTrieCursorFactory::new(
                        DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                        &trie_nodes_sorted,
                    );
                    let hashed_state = HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                        &hashed_state_sorted,
                    );
                    Ok(StorageRoot::new_hashed(
                        trie_cursor_factory,
                        hashed_state,
                        hashed_address,
                        #[cfg(feature = "metrics")]
                        metrics,
                    )
                    .with_prefix_set(prefix_set)
                    .calculate(retain_updates)?)
                })();
                let _ = tx.send(result);
            });
            storage_roots.insert(hashed_address, rx);
        }

        trace!(target: "trie::parallel_state_root", "calculating state root");
        let mut trie_updates = TrieUpdates::default();

        let provider_ro = self.view.provider_ro()?;
        let trie_cursor_factory = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &trie_nodes_sorted,
        );
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &hashed_state_sorted,
        );

        let walker = TrieWalker::new(
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?,
            prefix_sets.account_prefix_set,
        )
        .with_deletions_retained(retain_updates);
        let mut account_node_iter = TrieNodeIter::new(
            walker,
            hashed_cursor_factory.hashed_account_cursor().map_err(ProviderError::Database)?,
        );

        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);
        let mut account_rlp = Vec::with_capacity(128);
        while let Some(node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
            match node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let (storage_root, _, updates) = match storage_roots.remove(&hashed_address) {
                        Some(rx) => rx.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(StorageRootError::Database(
                                reth_db::DatabaseError::Other(format!(
                                    "channel closed for {hashed_address}"
                                )),
                            ))
                        })??,
                        // Since we do not store all intermediate nodes in the database, there might
                        // be a possibility of re-adding a non-modified leaf to the hash builder.
                        None => {
                            tracker.inc_missed_leaves();
                            StorageRoot::new_hashed(
                                trie_cursor_factory.clone(),
                                hashed_cursor_factory.clone(),
                                hashed_address,
                                #[cfg(feature = "metrics")]
                                self.metrics.storage_trie.clone(),
                            )
                            .calculate(retain_updates)?
                        }
                    };

                    if retain_updates {
                        trie_updates.insert_storage_updates(hashed_address, updates);
                    }

                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);
                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                }
            }
        }

        let root = hash_builder.root();

        trie_updates.finalize(
            account_node_iter.walker,
            hash_builder,
            prefix_sets.destroyed_accounts,
        );

        let stats = tracker.finish();

        #[cfg(feature = "metrics")]
        self.metrics.record_state_trie(stats);

        trace!(
            target: "trie::parallel_state_root",
            %root,
            duration = ?stats.duration(),
            branches_added = stats.branches_added(),
            leaves_added = stats.leaves_added(),
            missed_leaves = stats.missed_leaves(),
            precomputed_storage_roots = stats.precomputed_storage_roots(),
            "calculated state root"
        );

        Ok((root, trie_updates))
    }
}

/// Error during parallel state root calculation.
#[derive(Error, Debug)]
pub enum ParallelStateRootError {
    /// Error while calculating storage root.
    #[error(transparent)]
    StorageRoot(#[from] StorageRootError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl From<ParallelStateRootError> for ProviderError {
    fn from(error: ParallelStateRootError) -> Self {
        match error {
            ParallelStateRootError::Provider(error) => error,
            ParallelStateRootError::StorageRoot(StorageRootError::Database(error)) => {
                Self::Database(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address, U256};
    use rand::Rng;
    use reth_primitives::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::{test_utils, HashedPostState, HashedStorage};

    #[tokio::test]
    async fn random_parallel_root() {
        let factory = create_test_provider_factory();
        let consistent_view = ConsistentDbView::new(factory.clone(), None);

        let mut rng = rand::thread_rng();
        let mut state = (0..100)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.gen::<u64>()), ..Default::default() };
                let mut storage = HashMap::<B256, U256>::default();
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
            .collect::<HashMap<_, _>>();

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

        assert_eq!(
            ParallelStateRoot::new(consistent_view.clone(), Default::default())
                .incremental_root()
                .unwrap(),
            test_utils::state_root(state.clone())
        );

        let mut hashed_state = HashedPostState::default();
        for (address, (account, storage)) in &mut state {
            let hashed_address = keccak256(address);

            let should_update_account = rng.gen_bool(0.5);
            if should_update_account {
                *account = Account { balance: U256::from(rng.gen::<u64>()), ..*account };
                hashed_state.accounts.insert(hashed_address, Some(*account));
            }

            let should_update_storage = rng.gen_bool(0.3);
            if should_update_storage {
                for (slot, value) in storage.iter_mut() {
                    let hashed_slot = keccak256(slot);
                    *value = U256::from(rng.gen::<u64>());
                    hashed_state
                        .storages
                        .entry(hashed_address)
                        .or_insert_with(|| HashedStorage::new(false))
                        .storage
                        .insert(hashed_slot, *value);
                }
            }
        }

        assert_eq!(
            ParallelStateRoot::new(consistent_view, TrieInput::from_state(hashed_state))
                .incremental_root()
                .unwrap(),
            test_utils::state_root(state)
        );
    }
}
