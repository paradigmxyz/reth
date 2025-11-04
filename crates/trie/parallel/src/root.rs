#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;
use crate::{stats::ParallelTrieTracker, storage_root_targets::StorageRootTargets};
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_execution_errors::StorageRootError;
use reth_provider::{DatabaseProviderROFactory, ProviderError};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSets,
    trie_cursor::TrieCursorFactory,
    updates::TrieUpdates,
    walker::TrieWalker,
    HashBuilder, Nibbles, StorageRoot, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use std::{
    collections::HashMap,
    sync::{mpsc, OnceLock},
    time::Duration,
};
use thiserror::Error;
use tokio::runtime::{Builder, Handle, Runtime};
use tracing::*;

/// Parallel incremental state root calculator.
///
/// The calculator starts off by launching tasks to compute storage roots.
/// Then, it immediately starts walking the state trie updating the necessary trie
/// nodes in the process. Upon encountering a leaf node, it will poll the storage root
/// task for the corresponding hashed address.
///
/// Note: This implementation only serves as a fallback for the sparse trie-based
/// state root calculation. The sparse trie approach is more efficient as it avoids traversing
/// the entire trie, only operating on the modified parts.
#[derive(Debug)]
pub struct ParallelStateRoot<Factory> {
    /// Factory for creating state providers.
    factory: Factory,
    // Prefix sets indicating which portions of the trie need to be recomputed.
    prefix_sets: TriePrefixSets,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<Factory> ParallelStateRoot<Factory> {
    /// Create new parallel state root calculator.
    pub fn new(factory: Factory, prefix_sets: TriePrefixSets) -> Self {
        Self {
            factory,
            prefix_sets,
            #[cfg(feature = "metrics")]
            metrics: ParallelStateRootMetrics::default(),
        }
    }
}

impl<Factory> ParallelStateRoot<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + 'static,
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

    /// Computes the state root by calculating storage roots in parallel for modified accounts,
    /// then walking the state trie to build the final state root hash.
    fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let mut tracker = ParallelTrieTracker::default();
        let storage_root_targets = StorageRootTargets::new(
            self.prefix_sets
                .account_prefix_set
                .iter()
                .map(|nibbles| B256::from_slice(&nibbles.pack())),
            self.prefix_sets.storage_prefix_sets,
        );

        // Pre-calculate storage roots in parallel for accounts which were changed.
        tracker.set_precomputed_storage_roots(storage_root_targets.len() as u64);
        debug!(target: "trie::parallel_state_root", len = storage_root_targets.len(), "pre-calculating storage roots");
        let mut storage_roots = HashMap::with_capacity(storage_root_targets.len());

        // Get runtime handle once outside the loop
        let handle = get_runtime_handle();

        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let factory = self.factory.clone();
            #[cfg(feature = "metrics")]
            let metrics = self.metrics.storage_trie.clone();

            let (tx, rx) = mpsc::sync_channel(1);

            // Spawn a blocking task to calculate account's storage root from database I/O
            drop(handle.spawn_blocking(move || {
                let result = (|| -> Result<_, ParallelStateRootError> {
                    let provider = factory.database_provider_ro()?;
                    Ok(StorageRoot::new_hashed(
                        &provider,
                        &provider,
                        hashed_address,
                        prefix_set,
                        #[cfg(feature = "metrics")]
                        metrics,
                    )
                    .calculate(retain_updates)?)
                })();
                let _ = tx.send(result);
            }));
            storage_roots.insert(hashed_address, rx);
        }

        trace!(target: "trie::parallel_state_root", "calculating state root");
        let mut trie_updates = TrieUpdates::default();

        let provider = self.factory.database_provider_ro()?;

        let walker = TrieWalker::<_>::state_trie(
            provider.account_trie_cursor().map_err(ProviderError::Database)?,
            self.prefix_sets.account_prefix_set,
        )
        .with_deletions_retained(retain_updates);
        let mut account_node_iter = TrieNodeIter::state_trie(
            walker,
            provider.hashed_account_cursor().map_err(ProviderError::Database)?,
        );

        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);
        let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
        while let Some(node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
            match node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let storage_root_result = match storage_roots.remove(&hashed_address) {
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
                            StorageRoot::new_hashed(
                                &provider,
                                &provider,
                                hashed_address,
                                Default::default(),
                                #[cfg(feature = "metrics")]
                                self.metrics.storage_trie.clone(),
                            )
                            .calculate(retain_updates)?
                        }
                    };

                    let (storage_root, _, updates) = match storage_root_result {
                        reth_trie::StorageRootProgress::Complete(root, _, updates) => (root, (), updates),
                        reth_trie::StorageRootProgress::Progress(..) => {
                            return Err(ParallelStateRootError::StorageRoot(
                                StorageRootError::Database(DatabaseError::Other(
                                    "StorageRoot returned Progress variant in parallel trie calculation".to_string()
                                ))
                            ))
                        }
                    };

                    if retain_updates {
                        trie_updates.insert_storage_updates(hashed_address, updates);
                    }

                    account_rlp.clear();
                    let account = account.into_trie_account(storage_root);
                    account.encode(&mut account_rlp as &mut dyn BufMut);
                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                }
            }
        }

        let root = hash_builder.root();

        let removed_keys = account_node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder, removed_keys, self.prefix_sets.destroyed_accounts);

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
            "Calculated state root"
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
    /// Other unspecified error.
    #[error("{_0}")]
    Other(String),
}

impl From<ParallelStateRootError> for ProviderError {
    fn from(error: ParallelStateRootError) -> Self {
        match error {
            ParallelStateRootError::Provider(error) => error,
            ParallelStateRootError::StorageRoot(storage_err) => storage_err.into(),
            ParallelStateRootError::Other(msg) => Self::other(ParallelStateRootError::Other(msg)),
        }
    }
}

impl From<alloy_rlp::Error> for ParallelStateRootError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Provider(ProviderError::Rlp(error))
    }
}

/// Gets or creates a tokio runtime handle for spawning blocking tasks.
/// This ensures we always have a runtime available for I/O operations.
fn get_runtime_handle() -> Handle {
    Handle::try_current().unwrap_or_else(|_| {
        // Create a new runtime if no runtime is available
        static RT: OnceLock<Runtime> = OnceLock::new();

        let rt = RT.get_or_init(|| {
            Builder::new_multi_thread()
                // Keep the threads alive for at least the block time (12 seconds) plus buffer.
                // This prevents the costly process of spawning new threads on every
                // new block, and instead reuses the existing threads.
                .thread_keep_alive(Duration::from_secs(15))
                .build()
                .expect("Failed to create tokio runtime")
        });

        rt.handle().clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address, U256};
    use rand::Rng;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::{test_utils, HashedPostState, HashedStorage};
    use std::sync::Arc;

    #[tokio::test]
    async fn random_parallel_root() {
        let factory = create_test_provider_factory();
        let mut overlay_factory =
            reth_provider::providers::OverlayStateProviderFactory::new(factory.clone());

        let mut rng = rand::rng();
        let mut state = (0..100)
            .map(|_| {
                let address = Address::random();
                let account =
                    Account { balance: U256::from(rng.random::<u64>()), ..Default::default() };
                let mut storage = HashMap::<B256, U256>::default();
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
            ParallelStateRoot::new(overlay_factory.clone(), Default::default())
                .incremental_root()
                .unwrap(),
            test_utils::state_root(state.clone())
        );

        let mut hashed_state = HashedPostState::default();
        for (address, (account, storage)) in &mut state {
            let hashed_address = keccak256(address);

            let should_update_account = rng.random_bool(0.5);
            if should_update_account {
                *account = Account { balance: U256::from(rng.random::<u64>()), ..*account };
                hashed_state.accounts.insert(hashed_address, Some(*account));
            }

            let should_update_storage = rng.random_bool(0.3);
            if should_update_storage {
                for (slot, value) in storage.iter_mut() {
                    let hashed_slot = keccak256(slot);
                    *value = U256::from(rng.random::<u64>());
                    hashed_state
                        .storages
                        .entry(hashed_address)
                        .or_insert_with(HashedStorage::default)
                        .storage
                        .insert(hashed_slot, *value);
                }
            }
        }

        let prefix_sets = hashed_state.construct_prefix_sets();
        overlay_factory =
            overlay_factory.with_hashed_state_overlay(Some(Arc::new(hashed_state.into_sorted())));

        assert_eq!(
            ParallelStateRoot::new(overlay_factory, prefix_sets.freeze())
                .incremental_root()
                .unwrap(),
            test_utils::state_root(state)
        );
    }
}
