use crate::{stats::ParallelTrieTracker, storage_root_targets::StorageRootTargets};
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_db::database::Database;
use reth_interfaces::trie::StorageRootError;
use reth_primitives::{
    trie::{HashBuilder, Nibbles, TrieAccount},
    B256,
};
use reth_provider::{providers::ConsistentDbView, DatabaseProviderFactory, ProviderError};
use reth_tasks::pool::BlockingTaskPool;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    node_iter::{AccountNode, AccountNodeIter},
    trie_cursor::TrieCursorFactory,
    updates::TrieUpdates,
    walker::TrieWalker,
    HashedPostState, StorageRoot,
};
use std::{collections::HashMap, sync::Arc};
use thiserror::Error;
use tracing::*;

#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;

/// Async state root calculator.
///
/// The calculator starts off by launching tasks to compute storage roots.
/// Then, it immediately starts walking the state trie updating the necessary trie
/// nodes in the process. Upon encountering a leaf node, it will poll the storage root
/// task for the corresponding hashed address.
///
/// Internally, the calculator uses [ConsistentDbView] since
/// it needs to rely on database state saying the same until
/// the last transaction is open.
/// See docs of using [ConsistentDbView] for caveats.
///
/// For sync usage, take a look at `ParallelStateRoot`.
#[derive(Debug)]
pub struct AsyncStateRoot<DB, Provider> {
    /// Consistent view of the database.
    view: ConsistentDbView<DB, Provider>,
    /// Blocking task pool.
    blocking_pool: BlockingTaskPool,
    /// Changed hashed state.
    hashed_state: HashedPostState,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<DB, Provider> AsyncStateRoot<DB, Provider> {
    /// Create new async state root calculator.
    pub fn new(
        view: ConsistentDbView<DB, Provider>,
        blocking_pool: BlockingTaskPool,
        hashed_state: HashedPostState,
    ) -> Self {
        Self {
            view,
            blocking_pool,
            hashed_state,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }
}

impl<DB, Provider> AsyncStateRoot<DB, Provider>
where
    DB: Database + Clone + 'static,
    Provider: DatabaseProviderFactory<DB> + Clone + Send + Sync + 'static,
{
    /// Calculate incremental state root asynchronously.
    pub async fn incremental_root(self) -> Result<B256, AsyncStateRootError> {
        self.calculate(false).await.map(|(root, _)| root)
    }

    /// Calculate incremental state root with updates asynchronously.
    pub async fn incremental_root_with_updates(
        self,
    ) -> Result<(B256, TrieUpdates), AsyncStateRootError> {
        self.calculate(true).await
    }

    async fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, TrieUpdates), AsyncStateRootError> {
        let mut tracker = ParallelTrieTracker::default();
        let prefix_sets = self.hashed_state.construct_prefix_sets();
        let storage_root_targets = StorageRootTargets::new(
            self.hashed_state.accounts.keys().copied(),
            prefix_sets.storage_prefix_sets,
        );
        let hashed_state_sorted = Arc::new(self.hashed_state.into_sorted());

        // Pre-calculate storage roots async for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets.len() as u64);
        debug!(target: "trie::async_state_root", len = storage_root_targets.len(), "pre-calculating storage roots");
        let mut storage_roots = HashMap::with_capacity(storage_root_targets.len());
        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let view = self.view.clone();
            let hashed_state_sorted = hashed_state_sorted.clone();
            #[cfg(feature = "metrics")]
            let metrics = self.metrics.storage_trie.clone();
            let handle =
                self.blocking_pool.spawn_fifo(move || -> Result<_, AsyncStateRootError> {
                    let provider = view.provider_ro()?;
                    Ok(StorageRoot::new_hashed(
                        provider.tx_ref(),
                        HashedPostStateCursorFactory::new(provider.tx_ref(), &hashed_state_sorted),
                        hashed_address,
                        #[cfg(feature = "metrics")]
                        metrics,
                    )
                    .with_prefix_set(prefix_set)
                    .calculate(retain_updates)?)
                });
            storage_roots.insert(hashed_address, handle);
        }

        trace!(target: "trie::async_state_root", "calculating state root");
        let mut trie_updates = TrieUpdates::default();

        let provider_ro = self.view.provider_ro()?;
        let tx = provider_ro.tx_ref();
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(tx, &hashed_state_sorted);
        let trie_cursor_factory = tx;

        let trie_cursor =
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database)?;

        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);
        let walker = TrieWalker::new(trie_cursor, prefix_sets.account_prefix_set)
            .with_updates(retain_updates);
        let mut account_node_iter =
            AccountNodeIter::from_factory(walker, hashed_cursor_factory.clone())
                .map_err(ProviderError::Database)?;

        let mut account_rlp = Vec::with_capacity(128);
        while let Some(node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
            match node {
                AccountNode::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                AccountNode::Leaf(hashed_address, account) => {
                    let (storage_root, _, updates) = match storage_roots.remove(&hashed_address) {
                        Some(rx) => rx.await.map_err(|_| {
                            AsyncStateRootError::StorageRootChannelClosed { hashed_address }
                        })??,
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
            target: "trie::async_state_root",
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

/// Error during async state root calculation.
#[derive(Error, Debug)]
pub enum AsyncStateRootError {
    /// Storage root channel for a given address was closed.
    #[error("storage root channel for {hashed_address} got closed")]
    StorageRootChannelClosed {
        /// The hashed address for which channel was closed.
        hashed_address: B256,
    },
    /// Error while calculating storage root.
    #[error(transparent)]
    StorageRoot(#[from] StorageRootError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use rayon::ThreadPoolBuilder;
    use reth_primitives::{keccak256, Account, Address, StorageEntry, U256};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::{test_utils, HashedStorage};

    #[tokio::test]
    async fn random_async_root() {
        let blocking_pool = BlockingTaskPool::new(ThreadPoolBuilder::default().build().unwrap());

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
            AsyncStateRoot::new(
                consistent_view.clone(),
                blocking_pool.clone(),
                HashedPostState::default()
            )
            .incremental_root()
            .await
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
            AsyncStateRoot::new(consistent_view.clone(), blocking_pool.clone(), hashed_state)
                .incremental_root()
                .await
                .unwrap(),
            test_utils::state_root(state)
        );
    }
}
