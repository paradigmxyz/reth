#[cfg(feature = "metrics")]
use crate::metrics::ParallelStateRootMetrics;
use crate::{stats::ParallelTrieTracker, storage_root_targets::StorageRootTargets};
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use itertools::Itertools;
use reth_execution_errors::{SparseTrieError, StateProofError, StorageRootError};
use reth_primitives_traits::Account;
use reth_provider::{DatabaseProviderROFactory, ProviderError};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSets,
    trie_cursor::TrieCursorFactory,
    updates::{StorageTrieUpdates, TrieUpdates},
    walker::TrieWalker,
    HashBuilder, Nibbles, StorageRoot, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use std::{
    collections::BTreeMap,
    sync::mpsc,
    time::{Duration, Instant},
};
use thiserror::Error;
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
        self.calculate(false).map(|(root, _, _)| root)
    }

    /// Calculate incremental state root with updates in parallel.
    pub fn incremental_root_with_updates(
        self,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        self.calculate(true).map(|(root, _, updates)| (root, updates))
    }

    /// Computes the state root by calculating storage roots in parallel for modified accounts,
    /// then walking the state trie to build the final state root hash.
    fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, usize, TrieUpdates), ParallelStateRootError> {
        let Self {
            factory,
            prefix_sets,
            #[cfg(feature = "metrics")]
            metrics,
        } = self;

        let mut tracker = ParallelTrieTracker::default();
        let total_start = Instant::now();
        let mut state_root_ctx = ParallelStateRootContext::new();
        let storage_prefix_sets = prefix_sets.storage_prefix_sets.clone();
        let storage_root_targets = StorageRootTargets::new(
            prefix_sets.account_prefix_set.iter().map(|nibbles| B256::from_slice(&nibbles.pack())),
            prefix_sets.storage_prefix_sets,
        );
        let total_storage_root_targets = storage_root_targets.len();

        tracker.set_precomputed_storage_roots(total_storage_root_targets as u64);
        debug!(
            target: "trie::parallel_state_root",
            len = total_storage_root_targets,
            "pre-calculating storage roots"
        );
        let mut storage_roots = BTreeMap::new();
        let mut spawn_duration = Duration::ZERO;

        // Eagerly dispatch every modified-storage target onto the global rayon pool. Rayon's
        // work-stealing scheduler caps actual parallelism at ~CPU-cores, so queueing all targets
        // up front is bounded in practice and matches the parallelism style of `sender_recovery`
        // and `hashing_storage`.
        for (hashed_address, prefix_set) in
            storage_root_targets.into_iter().sorted_unstable_by_key(|(address, _)| *address)
        {
            let factory = factory.clone();
            #[cfg(feature = "metrics")]
            let metrics = metrics.storage_trie.clone();

            let (tx, rx) = mpsc::sync_channel(1);
            let spawn_start = Instant::now();
            rayon::spawn(move || {
                let task_start = Instant::now();
                let result = (|| -> Result<_, ParallelStateRootError> {
                    let provider = factory.database_provider_ro()?;
                    let storage_root = StorageRoot::new_hashed(
                        &provider,
                        &provider,
                        hashed_address,
                        prefix_set,
                        #[cfg(feature = "metrics")]
                        metrics,
                    );
                    Ok(if retain_updates {
                        storage_root.root_with_updates()?
                    } else {
                        (storage_root.root()?, 0, Default::default())
                    })
                })();
                let _ = tx.send(StorageRootTaskOutput { result, duration: task_start.elapsed() });
            });
            spawn_duration += spawn_start.elapsed();
            storage_roots.insert(hashed_address, rx);
        }

        trace!(target: "trie::parallel_state_root", "calculating state root");
        let provider = factory.database_provider_ro()?;

        let walker = TrieWalker::<_>::state_trie(
            provider.account_trie_cursor().map_err(ProviderError::Database)?,
            prefix_sets.account_prefix_set,
        )
        .with_deletions_retained(retain_updates);
        let mut account_node_iter = TrieNodeIter::state_trie(
            walker,
            provider.hashed_account_cursor().map_err(ProviderError::Database)?,
        );
        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);

        let account_walk_start = Instant::now();
        let mut recv_wait_duration = Duration::ZERO;
        let mut received_task_duration = Duration::ZERO;
        let mut max_task_duration = Duration::ZERO;
        let mut precomputed_storage_roots = 0usize;
        let mut fallback_storage_roots = 0usize;

        while let Some(node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
            match node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    // Drop receivers for accounts the walker has already passed: their results
                    // are no longer needed and holding the receivers would let the corresponding
                    // workers' completion sit in memory until end-of-walk.
                    while let Some((storage_hashed_address, _)) = storage_roots.first_key_value() {
                        if *storage_hashed_address >= hashed_address {
                            break;
                        }
                        storage_roots.pop_first();
                    }

                    let (storage_root, walked, storage_updates) = if let Some(rx) =
                        storage_roots.remove(&hashed_address)
                    {
                        precomputed_storage_roots += 1;
                        recv_storage_root_task_output(
                            hashed_address,
                            rx,
                            &mut recv_wait_duration,
                            &mut received_task_duration,
                            &mut max_task_duration,
                        )?
                    } else {
                        fallback_storage_roots += 1;
                        tracker.inc_missed_leaves();
                        // Defense-in-depth: if no worker ran this account's storage root, fall
                        // back to the real prefix set. Using an empty `PrefixSet` would return
                        // the on-disk root and drop this chunk's storage updates.
                        let fallback_prefix_set =
                            storage_prefix_sets.get(&hashed_address).cloned().unwrap_or_default();
                        StorageRoot::new_hashed(
                            &provider,
                            &provider,
                            hashed_address,
                            fallback_prefix_set,
                            #[cfg(feature = "metrics")]
                            metrics.storage_trie.clone(),
                        )
                        .root_with_updates()?
                    };

                    state_root_ctx.add_leaf(
                        hashed_address,
                        account,
                        storage_root,
                        walked,
                        storage_updates,
                        &mut hash_builder,
                        retain_updates,
                    )?;
                }
            }
        }
        let account_walk_duration = account_walk_start.elapsed();

        let root_start = Instant::now();
        let root = hash_builder.root();
        let root_duration = root_start.elapsed();

        let finalize_start = Instant::now();
        let removed_keys = account_node_iter.walker.take_removed_keys();
        let ParallelStateRootContext { mut trie_updates, hashed_entries_walked, .. } =
            state_root_ctx;
        trie_updates.finalize(hash_builder, removed_keys, prefix_sets.destroyed_accounts);
        let finalize_duration = finalize_start.elapsed();

        let stats = tracker.finish();
        let total_duration = total_start.elapsed();

        #[cfg(feature = "metrics")]
        metrics.record_state_trie(stats);

        debug!(
            target: "trie::parallel_state_root",
            spawned_tasks = total_storage_root_targets,
            precomputed_storage_roots,
            fallback_storage_roots,
            ?spawn_duration,
            ?account_walk_duration,
            ?recv_wait_duration,
            ?received_task_duration,
            ?max_task_duration,
            ?root_duration,
            ?finalize_duration,
            ?total_duration,
            "Calculated parallel state root timings"
        );

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

        Ok((root, hashed_entries_walked, trie_updates))
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
    /// Sparse trie error.
    #[error(transparent)]
    SparseTrie(#[from] SparseTrieError),
    /// Other unspecified error.
    #[error("{_0}")]
    Other(String),
}

impl From<ParallelStateRootError> for ProviderError {
    fn from(error: ParallelStateRootError) -> Self {
        match error {
            ParallelStateRootError::Provider(error) => error,
            ParallelStateRootError::StorageRoot(StorageRootError::Database(error)) => {
                Self::Database(error)
            }
            ParallelStateRootError::SparseTrie(error) => Self::other(error),
            ParallelStateRootError::Other(other) => Self::Database(DatabaseError::Other(other)),
        }
    }
}

impl From<alloy_rlp::Error> for ParallelStateRootError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Provider(ProviderError::Rlp(error))
    }
}

impl From<StateProofError> for ParallelStateRootError {
    fn from(error: StateProofError) -> Self {
        match error {
            StateProofError::Database(err) => Self::Provider(ProviderError::Database(err)),
            StateProofError::Rlp(err) => Self::Provider(ProviderError::Rlp(err)),
            StateProofError::TrieInconsistency(msg) => {
                Self::Provider(ProviderError::TrieWitnessError(msg))
            }
        }
    }
}

struct StorageRootTaskOutput {
    result: Result<(B256, usize, StorageTrieUpdates), ParallelStateRootError>,
    duration: Duration,
}

/// Mutable state threaded through the account walker: reusable encoding buffer, accumulated
/// trie updates, and running counters recorded into the final stats.
#[derive(Debug)]
struct ParallelStateRootContext {
    /// Reusable buffer for encoding account data.
    account_rlp: Vec<u8>,
    /// Accumulates updates from account and storage root calculation.
    trie_updates: TrieUpdates,
    /// Tracks total hashed entries walked.
    hashed_entries_walked: usize,
}

impl ParallelStateRootContext {
    fn new() -> Self {
        Self {
            account_rlp: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            trie_updates: TrieUpdates::default(),
            hashed_entries_walked: 0,
        }
    }

    /// Emits a leaf node into the hash builder after the storage root for the corresponding
    /// account has been produced (either by a worker or by the fallback path).
    #[allow(clippy::too_many_arguments)]
    fn add_leaf(
        &mut self,
        hashed_address: B256,
        account: Account,
        storage_root: B256,
        storage_slots_walked: usize,
        storage_updates: StorageTrieUpdates,
        hash_builder: &mut HashBuilder,
        retain_updates: bool,
    ) -> Result<(), ParallelStateRootError> {
        self.hashed_entries_walked += storage_slots_walked;
        if retain_updates {
            self.trie_updates.insert_storage_updates(hashed_address, storage_updates);
        }

        self.account_rlp.clear();
        let trie_account = account.into_trie_account(storage_root);
        trie_account.encode(&mut self.account_rlp as &mut dyn BufMut);
        hash_builder.add_leaf(Nibbles::unpack(hashed_address), &self.account_rlp);

        Ok(())
    }
}

fn recv_storage_root_task_output(
    hashed_address: B256,
    rx: mpsc::Receiver<StorageRootTaskOutput>,
    recv_wait_duration: &mut Duration,
    received_task_duration: &mut Duration,
    max_task_duration: &mut Duration,
) -> Result<(B256, usize, StorageTrieUpdates), ParallelStateRootError> {
    let recv_start = Instant::now();
    let output = rx.recv().map_err(|_| {
        ParallelStateRootError::StorageRoot(StorageRootError::Database(DatabaseError::Other(
            format!("channel closed for {hashed_address}"),
        )))
    })?;
    *recv_wait_duration += recv_start.elapsed();
    *received_task_duration += output.duration;
    *max_task_duration = (*max_task_duration).max(output.duration);
    output.result
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address, U256};
    use rand::Rng;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::{test_utils, HashedPostState, HashedStorage};
    use std::{collections::HashMap, sync::Arc};

    #[tokio::test]
    async fn random_parallel_root() {
        let factory = create_test_provider_factory();
        let changeset_cache = reth_trie_db::ChangesetCache::new();
        let mut overlay_factory = reth_provider::providers::OverlayStateProviderFactory::new(
            factory.clone(),
            changeset_cache,
        );

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
