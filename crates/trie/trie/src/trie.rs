use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSet, TriePrefixSets},
    progress::{IntermediateStateRootState, StateRootProgress},
    stats::TrieTracker,
    trie_cursor::TrieCursorFactory,
    updates::{StorageTrieUpdates, TrieUpdates},
    walker::TrieWalker,
    HashBuilder, Nibbles, TrieAccount,
};
use alloy_primitives::{keccak256, Address, B256};
use alloy_rlp::{BufMut, Encodable};
use reth_execution_errors::{StateRootError, StorageRootError};
use reth_primitives::constants::EMPTY_ROOT_HASH;
use tracing::trace;

#[cfg(feature = "metrics")]
use crate::metrics::{StateRootMetrics, TrieRootMetrics};

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
                let hash_builder = state.hash_builder.with_updates(retain_updates);
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

        let mut account_rlp = Vec::with_capacity(128);
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

                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);
                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);

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
                            hash_builder,
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

        trie_updates.finalize(
            account_node_iter.walker,
            hash_builder,
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
            keccak256(address),
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
                    tracker.inc_leaf();
                    hash_builder.add_leaf(
                        Nibbles::unpack(hashed_slot),
                        alloy_rlp::encode_fixed_size(&value).as_ref(),
                    );
                }
            }
        }

        let root = hash_builder.root();

        let mut trie_updates = StorageTrieUpdates::default();
        trie_updates.finalize(storage_node_iter.walker, hash_builder);

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
