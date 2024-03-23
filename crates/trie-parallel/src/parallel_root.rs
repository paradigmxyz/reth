use crate::{stats::ParallelTrieTracker, storage_root_targets::StorageRootTargets};
use alloy_rlp::{BufMut, Encodable};
use rayon::prelude::*;
use reth_db::database::Database;
use reth_interfaces::trie::StorageRootError;
use reth_primitives::{
    trie::{HashBuilder, Nibbles, TrieAccount},
    B256,
};
use reth_provider::{providers::ConsistentDbView, DatabaseProviderFactory, ProviderError};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{AccountNode, AccountNodeIter},
    trie_cursor::TrieCursorFactory,
    updates::TrieUpdates,
    walker::TrieWalker,
    HashedPostState, StorageRoot,
};
use std::collections::HashMap;
use thiserror::Error;
use tracing::*;

#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;

/// Parallel incremental state root calculator.
///
/// The calculator starts off by pre-computing storage roots of changed
/// accounts in parallel. Once that's done, it proceeds to walking the state
/// trie retrieving the pre-computed storage roots when needed.
///
/// Internally, the calculator uses [ConsistentDbView] since
/// it needs to rely on database state saying the same until
/// the last transaction is open.
/// See docs of using [ConsistentDbView] for caveats.
///
/// If possible, use more optimized `AsyncStateRoot` instead.
#[derive(Debug)]
pub struct ParallelStateRoot<DB, Provider> {
    /// Consistent view of the database.
    view: ConsistentDbView<DB, Provider>,
    /// Changed hashed state.
    hashed_state: HashedPostState,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<DB, Provider> ParallelStateRoot<DB, Provider> {
    /// Create new parallel state root calculator.
    pub fn new(view: ConsistentDbView<DB, Provider>, hashed_state: HashedPostState) -> Self {
        Self {
            view,
            hashed_state,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }
}

impl<DB, Provider> ParallelStateRoot<DB, Provider>
where
    DB: Database,
    Provider: DatabaseProviderFactory<DB> + Send + Sync,
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
        let prefix_sets = self.hashed_state.construct_prefix_sets();
        let storage_root_targets = StorageRootTargets::new(
            self.hashed_state.accounts.keys().copied(),
            prefix_sets.storage_prefix_sets,
        );
        let hashed_state_sorted = self.hashed_state.into_sorted();

        // Pre-calculate storage roots in parallel for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets.len() as u64);
        debug!(target: "trie::parallel_state_root", len = storage_root_targets.len(), "pre-calculating storage roots");
        let mut storage_roots = storage_root_targets
            .into_par_iter()
            .map(|(hashed_address, prefix_set)| {
                let provider_ro = self.view.provider_ro()?;
                let storage_root_result = StorageRoot::new_hashed(
                    provider_ro.tx_ref(),
                    HashedPostStateCursorFactory::new(provider_ro.tx_ref(), &hashed_state_sorted),
                    hashed_address,
                    #[cfg(feature = "metrics")]
                    self.metrics.storage_trie.clone(),
                )
                .with_prefix_set(prefix_set)
                .calculate(retain_updates);
                Ok((hashed_address, storage_root_result?))
            })
            .collect::<Result<HashMap<_, _>, ParallelStateRootError>>()?;

        trace!(target: "trie::parallel_state_root", "calculating state root");
        let mut trie_updates = TrieUpdates::default();

        let provider_ro = self.view.provider_ro()?;
        let hashed_cursor_factory =
            HashedPostStateCursorFactory::new(provider_ro.tx_ref(), &hashed_state_sorted);
        let trie_cursor_factory = provider_ro.tx_ref();

        let hashed_account_cursor =
            hashed_cursor_factory.hashed_account_cursor().map_err(ProviderError::Database)?;
        let trie_cursor =
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?;

        let walker = TrieWalker::new(trie_cursor, prefix_sets.account_prefix_set)
            .with_updates(retain_updates);
        let mut account_node_iter = AccountNodeIter::new(walker, hashed_account_cursor);
        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);

        let mut account_rlp = Vec::with_capacity(128);
        while let Some(node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
            match node {
                AccountNode::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                AccountNode::Leaf(hashed_address, account) => {
                    let (storage_root, _, updates) = match storage_roots.remove(&hashed_address) {
                        Some(result) => result,
                        // Since we do not store all intermediate nodes in the database, there might
                        // be a possibility of re-adding a non-modified leaf to the hash builder.
                        None => {
                            tracker.inc_missed_leaves();
                            StorageRoot::new_hashed(
                                trie_cursor_factory,
                                hashed_cursor_factory.clone(),
                                hashed_address,
                                #[cfg(feature = "metrics")]
                                self.metrics.storage_trie.clone(),
                            )
                            .calculate(retain_updates)?
                        }
                    };

                    if retain_updates {
                        trie_updates.extend(updates.into_iter());
                    }

                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);
                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                }
            }
        }

        let root = hash_builder.root();

        trie_updates.finalize_state_updates(
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
            ParallelStateRootError::StorageRoot(StorageRootError::DB(error)) => {
                ProviderError::Database(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use reth_primitives::{keccak256, Account, Address, StorageEntry, U256};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::{test_utils, HashedStorage};

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
            ParallelStateRoot::new(consistent_view.clone(), HashedPostState::default())
                .incremental_root()
                .unwrap(),
            test_utils::state_root(state.clone())
        );

        let mut hashed_state = HashedPostState::default();
        for (address, (account, storage)) in state.iter_mut() {
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
            ParallelStateRoot::new(consistent_view, hashed_state).incremental_root().unwrap(),
            test_utils::state_root(state)
        );
    }
}
