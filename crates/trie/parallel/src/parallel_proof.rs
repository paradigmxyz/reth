use crate::{
    parallel_root::ParallelStateRootError, stats::ParallelTrieTracker, StorageRootTargets,
};
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_db::DatabaseError;
use reth_execution_errors::StorageRootError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory, ProviderError,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSetsMut},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    walker::TrieWalker,
    HashBuilder, MultiProof, Nibbles, StorageMultiProof, TrieAccount, TrieInput,
};
use reth_trie_common::proof::ProofRetainer;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tracing::debug;

#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;

/// TODO:
#[derive(Debug)]
pub struct ParallelProof<Factory> {
    /// Consistent view of the database.
    view: ConsistentDbView<Factory>,
    /// Trie input.
    input: TrieInput,
    /// Parallel state root metrics.
    #[cfg(feature = "metrics")]
    metrics: ParallelStateRootMetrics,
}

impl<Factory> ParallelProof<Factory> {
    /// Create new state proof generator.
    pub fn new(view: ConsistentDbView<Factory>, input: TrieInput) -> Self {
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

        let trie_nodes_sorted = Arc::new(self.input.nodes.into_sorted());
        let hashed_state_sorted = Arc::new(self.input.state.into_sorted());

        // Extend prefix sets with targets
        let mut prefix_sets = self.input.prefix_sets.clone();
        prefix_sets.extend(TriePrefixSetsMut {
            account_prefix_set: PrefixSetMut::from(targets.keys().map(Nibbles::unpack)),
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
            let targets = targets
                .get(&hashed_address)
                .map_or(Vec::new(), |slots| slots.iter().map(Nibbles::unpack).collect());

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

                    Ok(storage_multiproof(
                        trie_cursor_factory,
                        hashed_cursor_factory,
                        prefix_set,
                        hashed_address,
                        targets,
                    )
                    .map_err(ProviderError::Database)?)
                })();
                let _ = tx.send(result);
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
        let mut account_rlp = Vec::with_capacity(128);
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
                                reth_db::DatabaseError::Other(format!(
                                    "channel closed for {hashed_address}"
                                )),
                            ))
                        })??,
                        // Since we do not store all intermediate nodes in the database, there might
                        // be a possibility of re-adding a non-modified leaf to the hash builder.
                        None => {
                            tracker.inc_missed_leaves();
                            storage_multiproof(
                                trie_cursor_factory.clone(),
                                hashed_cursor_factory.clone(),
                                Default::default(), // TODO: check?
                                hashed_address,
                                targets.get(&hashed_address).map_or(Vec::new(), |slots| {
                                    slots.iter().map(Nibbles::unpack).collect()
                                }),
                            )
                            .map_err(ProviderError::Database)?
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

/// Generate a storage multiproof according to specified targets.
pub fn storage_multiproof<T, H>(
    trie_cursor_factory: T,
    hashed_cursor_factory: H,
    prefix_set: PrefixSet, // must already include targets
    hashed_address: B256,
    targets: Vec<Nibbles>,
) -> Result<StorageMultiProof, DatabaseError>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    let mut hashed_storage_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

    // short circuit on empty storage
    if hashed_storage_cursor.is_storage_empty()? {
        return Ok(StorageMultiProof::empty())
    }

    let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address)?;
    let walker = TrieWalker::new(trie_cursor, prefix_set);

    let retainer = ProofRetainer::from_iter(targets);
    let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
    let mut storage_node_iter = TrieNodeIter::new(walker, hashed_storage_cursor);
    while let Some(node) = storage_node_iter.try_next()? {
        match node {
            TrieElement::Branch(node) => {
                hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
            }
            TrieElement::Leaf(hashed_slot, value) => {
                hash_builder.add_leaf(
                    Nibbles::unpack(hashed_slot),
                    alloy_rlp::encode_fixed_size(&value).as_ref(),
                );
            }
        }
    }

    let root = hash_builder.root();
    Ok(StorageMultiProof { root, subtree: hash_builder.take_proof_nodes() })
}
