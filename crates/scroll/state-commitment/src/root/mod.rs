use super::{PoseidonKeyHasher, PoseidonValueHasher, ScrollTrieAccount};
use alloy_primitives::{Address, BlockNumber, B256};
use reth_db::transaction::DbTx;
use reth_execution_errors::{StateRootError, StorageRootError};
use reth_scroll_primitives::poseidon::EMPTY_ROOT_HASH;
use reth_scroll_trie::HashBuilder;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSet, TriePrefixSets},
    stats::TrieTracker,
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::{StorageTrieUpdates, TrieUpdates},
    walker::TrieWalker,
    BitsCompatibility, HashedPostState, HashedPostStateSorted, HashedStorage,
    IntermediateStateRootState, KeyHasher, Nibbles, StateRootProgress, TrieInput,
};
use tracing::{debug, trace};

#[cfg(feature = "metrics")]
use reth_trie::metrics::{StateRootMetrics, TrieRootMetrics, TrieType};

mod parallel;
pub use parallel::ParallelStateRoot;

mod utils;
pub use utils::*;

// TODO(scroll): Instead of introducing this new type we should make StateRoot generic over
// the [`HashBuilder`] and key traversal types

/// `StateRoot` is used to compute the root node of a state trie.
#[derive(Debug)]
pub struct StateRoot<T, H> {
    /// The factory for trie cursors.
    pub trie_cursor_factory: T,
    /// The factory for hashed cursors.
    pub hashed_cursor_factory: H,
    /// A set of prefix sets that have changed.
    pub prefix_sets: TriePrefixSets,
    /// Previous intermediate state.
    previous_state: Option<IntermediateStateRootState>,
    /// The number of updates after which the intermediate progress should be returned.
    threshold: u64,
    #[cfg(feature = "metrics")]
    /// State root metrics.
    metrics: StateRootMetrics,
}

impl<T, H> StateRoot<T, H> {
    /// Creates [`StateRoot`] with `trie_cursor_factory` and `hashed_cursor_factory`. All other
    /// parameters are set to reasonable defaults.
    ///
    /// The cursors created by given factories are then used to walk through the accounts and
    /// calculate the state root value with.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Self {
        Self {
            trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: TriePrefixSets::default(),
            previous_state: None,
            threshold: 100_000,
            #[cfg(feature = "metrics")]
            metrics: StateRootMetrics::default(),
        }
    }

    /// Set the prefix sets.
    pub fn with_prefix_sets(mut self, prefix_sets: TriePrefixSets) -> Self {
        self.prefix_sets = prefix_sets;
        self
    }

    /// Set the threshold.
    pub const fn with_threshold(mut self, threshold: u64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the threshold to maximum value so that intermediate progress is not returned.
    pub const fn with_no_threshold(mut self) -> Self {
        self.threshold = u64::MAX;
        self
    }

    /// Set the previously recorded intermediate state.
    pub fn with_intermediate_state(mut self, state: Option<IntermediateStateRootState>) -> Self {
        self.previous_state = state;
        self
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> StateRoot<T, HF> {
        StateRoot {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            threshold: self.threshold,
            previous_state: self.previous_state,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> StateRoot<TF, H> {
        StateRoot {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            threshold: self.threshold,
            previous_state: self.previous_state,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }
}

impl<T, H> StateRoot<T, H>
where
    T: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder. Collects the updates in the process.
    ///
    /// Ignores the threshold.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation and the trie updates.
    pub fn root_with_updates(self) -> Result<(B256, TrieUpdates), StateRootError> {
        match self.with_no_threshold().calculate(true)? {
            StateRootProgress::Complete(root, _, updates) => Ok((root, updates)),
            StateRootProgress::Progress(..) => unreachable!(), // unreachable threshold
        }
    }

    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder.
    ///
    /// # Returns
    ///
    /// The state root hash.
    pub fn root(self) -> Result<B256, StateRootError> {
        match self.calculate(false)? {
            StateRootProgress::Complete(root, _, _) => Ok(root),
            StateRootProgress::Progress(..) => unreachable!(), // update retenion is disabled
        }
    }

    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder. Collects the updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    pub fn root_with_progress(self) -> Result<StateRootProgress, StateRootError> {
        self.calculate(true)
    }

    fn calculate(self, retain_updates: bool) -> Result<StateRootProgress, StateRootError> {
        trace!(target: "trie::state_root", "calculating state root");
        let mut tracker = TrieTracker::default();
        let mut trie_updates = TrieUpdates::default();

        let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;

        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let (mut hash_builder, mut account_node_iter) = match self.previous_state {
            Some(state) => {
                let hash_builder = state.hash_builder.with_updates(retain_updates).into();

                let walker = TrieWalker::from_stack(
                    trie_cursor,
                    state.walker_stack,
                    self.prefix_sets.account_prefix_set,
                )
                .with_deletions_retained(retain_updates);
                let node_iter = TrieNodeIter::new(walker, hashed_account_cursor)
                    .with_last_hashed_key(state.last_account_key);
                (hash_builder, node_iter)
            }
            None => {
                let hash_builder = HashBuilder::default().with_updates(retain_updates);
                let walker = TrieWalker::new(trie_cursor, self.prefix_sets.account_prefix_set)
                    .with_deletions_retained(retain_updates);
                let node_iter = TrieNodeIter::new(walker, hashed_account_cursor);
                (hash_builder, node_iter)
            }
        };

        let mut hashed_entries_walked = 0;
        let mut updated_storage_nodes = 0;
        while let Some(node) = account_node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    tracker.inc_branch();
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    tracker.inc_leaf();
                    hashed_entries_walked += 1;

                    // We assume we can always calculate a storage root without
                    // OOMing. This opens us up to a potential DOS vector if
                    // a contract had too many storage entries and they were
                    // all buffered w/o us returning and committing our intermediate
                    // progress.
                    // TODO: We can consider introducing the TrieProgress::Progress/Complete
                    // abstraction inside StorageRoot, but let's give it a try as-is for now.
                    let storage_root_calculator = StorageRoot::new_hashed(
                        self.trie_cursor_factory.clone(),
                        self.hashed_cursor_factory.clone(),
                        hashed_address,
                        #[cfg(feature = "metrics")]
                        self.metrics.storage_trie.clone(),
                    )
                    .with_prefix_set(
                        self.prefix_sets
                            .storage_prefix_sets
                            .get(&hashed_address)
                            .cloned()
                            .unwrap_or_default(),
                    );

                    let storage_root = if retain_updates {
                        let (root, storage_slots_walked, updates) =
                            storage_root_calculator.root_with_updates()?;
                        hashed_entries_walked += storage_slots_walked;
                        // We only walk over hashed address once, so it's safe to insert.
                        updated_storage_nodes += updates.len();
                        trie_updates.insert_storage_updates(hashed_address, updates);
                        root
                    } else {
                        storage_root_calculator.root()?
                    };

                    let account = ScrollTrieAccount::from((account, storage_root));
                    let account_hash = PoseidonValueHasher::hash_account(account);
                    hash_builder
                        .add_leaf(Nibbles::unpack_bits(hashed_address), account_hash.as_slice());

                    // Decide if we need to return intermediate progress.
                    let total_updates_len = updated_storage_nodes +
                        account_node_iter.walker.removed_keys_len() +
                        hash_builder.updates_len();
                    if retain_updates && total_updates_len as u64 >= self.threshold {
                        let (walker_stack, walker_deleted_keys) = account_node_iter.walker.split();
                        trie_updates.removed_nodes.extend(walker_deleted_keys);
                        let (hash_builder, hash_builder_updates) = hash_builder.split();
                        trie_updates.account_nodes.extend(hash_builder_updates);

                        let state = IntermediateStateRootState {
                            hash_builder: hash_builder.into(),
                            walker_stack,
                            last_account_key: hashed_address,
                        };

                        return Ok(StateRootProgress::Progress(
                            Box::new(state),
                            hashed_entries_walked,
                            trie_updates,
                        ))
                    }
                }
            }
        }

        let root = hash_builder.root();

        let removed_keys = account_node_iter.walker.take_removed_keys();
        trie_updates.finalize(
            hash_builder.into(),
            removed_keys,
            self.prefix_sets.destroyed_accounts,
        );

        let stats = tracker.finish();

        #[cfg(feature = "metrics")]
        self.metrics.state_trie.record(stats);

        trace!(
            target: "trie::state_root",
            %root,
            duration = ?stats.duration(),
            branches_added = stats.branches_added(),
            leaves_added = stats.leaves_added(),
            "calculated state root"
        );

        Ok(StateRootProgress::Complete(root, hashed_entries_walked, trie_updates))
    }
}

/// `StorageRoot` is used to compute the root node of an account storage trie.
#[derive(Debug)]
pub struct StorageRoot<T, H> {
    /// A reference to the database transaction.
    pub trie_cursor_factory: T,
    /// The factory for hashed cursors.
    pub hashed_cursor_factory: H,
    /// The hashed address of an account.
    pub hashed_address: B256,
    /// The set of storage slot prefixes that have changed.
    pub prefix_set: PrefixSet,
    /// Storage root metrics.
    #[cfg(feature = "metrics")]
    metrics: TrieRootMetrics,
}

impl<T, H> StorageRoot<T, H> {
    /// Creates a new storage root calculator given a raw address.
    pub fn new(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        address: Address,
        #[cfg(feature = "metrics")] metrics: TrieRootMetrics,
    ) -> Self {
        Self::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            PoseidonKeyHasher::hash_key(address),
            #[cfg(feature = "metrics")]
            metrics,
        )
    }

    /// Creates a new storage root calculator given a hashed address.
    pub fn new_hashed(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        hashed_address: B256,
        #[cfg(feature = "metrics")] metrics: TrieRootMetrics,
    ) -> Self {
        Self {
            trie_cursor_factory,
            hashed_cursor_factory,
            hashed_address,
            prefix_set: PrefixSet::default(),
            #[cfg(feature = "metrics")]
            metrics,
        }
    }

    /// Set the changed prefixes.
    pub fn with_prefix_set(mut self, prefix_set: PrefixSet) -> Self {
        self.prefix_set = prefix_set;
        self
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> StorageRoot<T, HF> {
        StorageRoot {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            hashed_address: self.hashed_address,
            prefix_set: self.prefix_set,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> StorageRoot<TF, H> {
        StorageRoot {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            hashed_address: self.hashed_address,
            prefix_set: self.prefix_set,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }
}

impl<T, H> StorageRoot<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root and storage trie updates for a given address.
    pub fn root_with_updates(self) -> Result<(B256, usize, StorageTrieUpdates), StorageRootError> {
        self.calculate(true)
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root.
    pub fn root(self) -> Result<B256, StorageRootError> {
        let (root, _, _) = self.calculate(false)?;
        Ok(root)
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root, number of walked entries and trie updates
    /// for a given address if requested.
    pub fn calculate(
        self,
        retain_updates: bool,
    ) -> Result<(B256, usize, StorageTrieUpdates), StorageRootError> {
        trace!(target: "trie::storage_root", hashed_address = ?self.hashed_address, "calculating storage root");

        let mut hashed_storage_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(self.hashed_address)?;

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty()? {
            return Ok((EMPTY_ROOT_HASH, 0, StorageTrieUpdates::deleted()))
        }

        let mut tracker = TrieTracker::default();
        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(self.hashed_address)?;
        let walker =
            TrieWalker::new(trie_cursor, self.prefix_set).with_deletions_retained(retain_updates);

        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);

        let mut storage_node_iter = TrieNodeIter::new(walker, hashed_storage_cursor);
        while let Some(node) = storage_node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    tracker.inc_branch();
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_slot, value) => {
                    let hashed_value = PoseidonValueHasher::hash_storage(value);
                    tracker.inc_leaf();
                    hash_builder.add_leaf(Nibbles::unpack_bits(hashed_slot), hashed_value.as_ref());
                }
            }
        }

        let root = hash_builder.root();

        let mut trie_updates = StorageTrieUpdates::default();
        let removed_keys = storage_node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder.into(), removed_keys);

        let stats = tracker.finish();

        #[cfg(feature = "metrics")]
        self.metrics.record(stats);

        trace!(
            target: "trie::storage_root",
            %root,
            hashed_address = %self.hashed_address,
            duration = ?stats.duration(),
            branches_added = stats.branches_added(),
            leaves_added = stats.leaves_added(),
            "calculated storage root"
        );

        let storage_slots_walked = stats.leaves_added() as usize;
        Ok((root, storage_slots_walked, trie_updates))
    }
}

use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseStorageRoot, DatabaseTrieCursorFactory,
    PrefixSetLoader,
};
use std::ops::RangeInclusive;

impl<'a, TX: DbTx> DatabaseStateRoot<'a, TX>
    for StateRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(tx: &'a TX) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx), DatabaseHashedCursorFactory::new(tx))
    }

    fn incremental_root_calculator(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError> {
        let loaded_prefix_sets = PrefixSetLoader::<_, PoseidonKeyHasher>::new(tx).load(range)?;
        Ok(Self::from_tx(tx).with_prefix_sets(loaded_prefix_sets))
    }

    fn incremental_root(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<B256, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root()
    }

    fn incremental_root_with_updates(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root_with_updates()
    }

    fn incremental_root_with_progress(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root with progress");
        Self::incremental_root_calculator(tx, range)?.root_with_progress()
    }

    fn overlay_root(tx: &'a TX, post_state: HashedPostState) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates, HashedPostStateSorted), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        let (root, updates) = StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(prefix_sets)
        .root_with_updates()?;
        Ok((root, updates, state_sorted))
    }

    fn overlay_root_from_nodes(tx: &'a TX, input: TrieInput) -> Result<B256, StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), &nodes_sorted),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()
    }

    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), &nodes_sorted),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()
    }

    fn root(tx: &'a TX) -> Result<B256, StateRootError> {
        Self::from_tx(tx).root()
    }

    fn root_with_updates(tx: &'a TX) -> Result<(B256, TrieUpdates), StateRootError> {
        Self::from_tx(tx).root_with_updates()
    }

    fn root_from_prefix_sets_with_updates(
        tx: &'a TX,
        prefix_sets: TriePrefixSets,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        Self::from_tx(tx).with_prefix_sets(prefix_sets).root_with_updates()
    }

    fn root_with_progress(
        tx: &'a TX,
        state: Option<IntermediateStateRootState>,
    ) -> Result<StateRootProgress, StateRootError> {
        Self::from_tx(tx).with_intermediate_state(state).root_with_progress()
    }
}

impl<'a, TX: DbTx> DatabaseStorageRoot<'a, TX>
    for StorageRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }

    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self {
        Self::new_hashed(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            hashed_address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }

    fn overlay_root(
        tx: &'a TX,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted = HashedPostState::from_hashed_storage(
            PoseidonKeyHasher::hash_key(address),
            hashed_storage,
        )
        .into_sorted();
        StorageRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
            address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
        .with_prefix_set(prefix_set)
        .root()
    }
}
