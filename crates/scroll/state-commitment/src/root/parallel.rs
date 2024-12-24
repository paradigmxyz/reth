use crate::{PoseidonValueHasher, ScrollTrieAccount};
use alloy_primitives::B256;
use itertools::Itertools;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
};
use reth_storage_api::StateCommitmentProvider;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdates,
    walker::TrieWalker,
    BitsCompatibility, Nibbles, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
#[cfg(feature = "metrics")]
use reth_trie_parallel::metrics::ParallelStateRootMetrics;
use reth_trie_parallel::{
    root::ParallelStateRootError, stats::ParallelTrieTracker, StorageRootTargets,
};
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, trace};

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
    Factory: DatabaseProviderFactory<Provider: BlockReader>
        + StateCommitmentProvider
        + Clone
        + Send
        + Sync
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

    fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let mut tracker = ParallelTrieTracker::default();
        let trie_nodes_sorted = Arc::new(self.input.nodes.into_sorted());
        let hashed_state_sorted = Arc::new(self.input.state.into_sorted());
        let prefix_sets = self.input.prefix_sets.freeze();
        let storage_root_targets = StorageRootTargets::new(
            prefix_sets
                .account_prefix_set
                .iter()
                .map(|nibbles| B256::from_slice(&nibbles.pack_bits())),
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
                    Ok(super::StorageRoot::new_hashed(
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

        let mut hash_builder = super::HashBuilder::default().with_updates(retain_updates);
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
                            super::StorageRoot::new_hashed(
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

                    let account = ScrollTrieAccount::from((account, storage_root));
                    let account_hash = PoseidonValueHasher::hash_account(account);
                    hash_builder
                        .add_leaf(Nibbles::unpack_bits(hashed_address), account_hash.as_slice());
                }
            }
        }

        let root = hash_builder.root();

        let removed_keys = account_node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder.into(), removed_keys, prefix_sets.destroyed_accounts);

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
