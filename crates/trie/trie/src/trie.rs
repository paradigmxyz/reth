use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSet, TriePrefixSets},
    progress::{
        IntermediateRootState, IntermediateStateRootState, IntermediateStorageRootState,
        StateRootProgress, StorageRootProgress,
    },
    stats::TrieTracker,
    trie_cursor::{TrieCursor, TrieCursorFactory},
    updates::{StorageTrieUpdates, TrieUpdates},
    walker::TrieWalker,
    HashBuilder, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{keccak256, Address, B256};
use alloy_rlp::{BufMut, Encodable};
use alloy_trie::proof::AddedRemovedKeys;
use reth_execution_errors::{StateRootError, StorageRootError};
use reth_primitives_traits::Account;
use tracing::{debug, instrument, trace};

/// The default updates after which root algorithms should return intermediate progress rather than
/// finishing the computation.
const DEFAULT_INTERMEDIATE_THRESHOLD: u64 = 100_000;

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
            threshold: DEFAULT_INTERMEDIATE_THRESHOLD,
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
    /// The state root and the trie updates.
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
            StateRootProgress::Progress(..) => unreachable!(), // update retention is disabled
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

        let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;
        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;

        // create state root context once for reuse
        let mut storage_ctx = StateRootContext::new();

        // first handle any in-progress storage root calculation
        let (mut hash_builder, mut account_node_iter) = if let Some(state) = self.previous_state {
            let IntermediateStateRootState { account_root_state, storage_root_state } = state;

            // resume account trie iteration
            let mut hash_builder = account_root_state.hash_builder.with_updates(retain_updates);
            let walker = TrieWalker::<_>::state_trie_from_stack(
                trie_cursor,
                account_root_state.walker_stack,
                self.prefix_sets.account_prefix_set,
            )
            .with_deletions_retained(retain_updates);
            let account_node_iter = TrieNodeIter::state_trie(walker, hashed_account_cursor)
                .with_last_hashed_key(account_root_state.last_hashed_key);

            // if we have an in-progress storage root, complete it first
            if let Some(storage_state) = storage_root_state {
                let hashed_address = account_root_state.last_hashed_key;
                let account = storage_state.account;

                debug!(
                    target: "trie::state_root",
                    account_nonce = account.nonce,
                    account_balance = ?account.balance,
                    last_hashed_key = ?account_root_state.last_hashed_key,
                    "Resuming storage root calculation"
                );

                // resume the storage root calculation
                let remaining_threshold = self.threshold.saturating_sub(
                    storage_ctx.total_updates_len(&account_node_iter, &hash_builder),
                );

                let storage_root_calculator = StorageRoot::new_hashed(
                    self.trie_cursor_factory.clone(),
                    self.hashed_cursor_factory.clone(),
                    hashed_address,
                    self.prefix_sets
                        .storage_prefix_sets
                        .get(&hashed_address)
                        .cloned()
                        .unwrap_or_default(),
                    #[cfg(feature = "metrics")]
                    self.metrics.storage_trie.clone(),
                )
                .with_intermediate_state(Some(storage_state.state))
                .with_threshold(remaining_threshold);

                let storage_result = storage_root_calculator.calculate(retain_updates)?;
                if let Some(storage_state) = storage_ctx.process_storage_root_result(
                    storage_result,
                    hashed_address,
                    account,
                    &mut hash_builder,
                    retain_updates,
                )? {
                    // still in progress, need to pause again
                    return Ok(storage_ctx.create_progress_state(
                        account_node_iter,
                        hash_builder,
                        account_root_state.last_hashed_key,
                        Some(storage_state),
                    ));
                }
            }

            (hash_builder, account_node_iter)
        } else {
            // no intermediate state, create new hash builder and node iter for state root
            // calculation
            let hash_builder = HashBuilder::default().with_updates(retain_updates);
            let walker = TrieWalker::state_trie(trie_cursor, self.prefix_sets.account_prefix_set)
                .with_deletions_retained(retain_updates);
            let node_iter = TrieNodeIter::state_trie(walker, hashed_account_cursor);
            (hash_builder, node_iter)
        };

        while let Some(node) = account_node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    tracker.inc_branch();
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    tracker.inc_leaf();
                    storage_ctx.hashed_entries_walked += 1;

                    // calculate storage root, calculating the remaining threshold so we have
                    // bounded memory usage even while in the middle of storage root calculation
                    let remaining_threshold = self.threshold.saturating_sub(
                        storage_ctx.total_updates_len(&account_node_iter, &hash_builder),
                    );

                    let storage_root_calculator = StorageRoot::new_hashed(
                        self.trie_cursor_factory.clone(),
                        self.hashed_cursor_factory.clone(),
                        hashed_address,
                        self.prefix_sets
                            .storage_prefix_sets
                            .get(&hashed_address)
                            .cloned()
                            .unwrap_or_default(),
                        #[cfg(feature = "metrics")]
                        self.metrics.storage_trie.clone(),
                    )
                    .with_threshold(remaining_threshold);

                    let storage_result = storage_root_calculator.calculate(retain_updates)?;
                    if let Some(storage_state) = storage_ctx.process_storage_root_result(
                        storage_result,
                        hashed_address,
                        account,
                        &mut hash_builder,
                        retain_updates,
                    )? {
                        // storage root hit threshold, need to pause
                        return Ok(storage_ctx.create_progress_state(
                            account_node_iter,
                            hash_builder,
                            hashed_address,
                            Some(storage_state),
                        ));
                    }

                    // decide if we need to return intermediate progress
                    let total_updates_len =
                        storage_ctx.total_updates_len(&account_node_iter, &hash_builder);
                    if retain_updates && total_updates_len >= self.threshold {
                        return Ok(storage_ctx.create_progress_state(
                            account_node_iter,
                            hash_builder,
                            hashed_address,
                            None,
                        ));
                    }
                }
            }
        }

        let root = hash_builder.root();

        let removed_keys = account_node_iter.walker.take_removed_keys();
        let StateRootContext { mut trie_updates, hashed_entries_walked, .. } = storage_ctx;
        trie_updates.finalize(hash_builder, removed_keys, self.prefix_sets.destroyed_accounts);

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

/// Contains state mutated during state root calculation and storage root result handling.
#[derive(Debug)]
pub(crate) struct StateRootContext {
    /// Reusable buffer for encoding account data.
    account_rlp: Vec<u8>,
    /// Accumulates updates from account and storage root calculation.
    trie_updates: TrieUpdates,
    /// Tracks total hashed entries walked.
    hashed_entries_walked: usize,
    /// Counts storage trie nodes updated.
    updated_storage_nodes: usize,
}

impl StateRootContext {
    /// Creates a new state root context.
    fn new() -> Self {
        Self {
            account_rlp: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            trie_updates: TrieUpdates::default(),
            hashed_entries_walked: 0,
            updated_storage_nodes: 0,
        }
    }

    /// Creates a [`StateRootProgress`] when the threshold is hit, from the state of the current
    /// [`TrieNodeIter`], [`HashBuilder`], last hashed key and any storage root intermediate state.
    fn create_progress_state<C, H, K>(
        mut self,
        account_node_iter: TrieNodeIter<C, H, K>,
        hash_builder: HashBuilder,
        last_hashed_key: B256,
        storage_state: Option<IntermediateStorageRootState>,
    ) -> StateRootProgress
    where
        C: TrieCursor,
        H: HashedCursor,
        K: AsRef<AddedRemovedKeys>,
    {
        let (walker_stack, walker_deleted_keys) = account_node_iter.walker.split();
        self.trie_updates.removed_nodes.extend(walker_deleted_keys);
        let (hash_builder, hash_builder_updates) = hash_builder.split();
        self.trie_updates.account_nodes.extend(hash_builder_updates);

        let account_state = IntermediateRootState { hash_builder, walker_stack, last_hashed_key };

        let state = IntermediateStateRootState {
            account_root_state: account_state,
            storage_root_state: storage_state,
        };

        StateRootProgress::Progress(Box::new(state), self.hashed_entries_walked, self.trie_updates)
    }

    /// Calculates the total number of updated nodes.
    fn total_updates_len<C, H, K>(
        &self,
        account_node_iter: &TrieNodeIter<C, H, K>,
        hash_builder: &HashBuilder,
    ) -> u64
    where
        C: TrieCursor,
        H: HashedCursor,
        K: AsRef<AddedRemovedKeys>,
    {
        (self.updated_storage_nodes
            + account_node_iter.walker.removed_keys_len()
            + hash_builder.updates_len()) as u64
    }

    /// Processes the result of a storage root calculation.
    ///
    /// Handles both completed and in-progress storage root calculations:
    /// - For completed roots: encodes the account with the storage root, updates the hash builder
    ///   with the new account, and updates metrics.
    /// - For in-progress roots: returns the intermediate state for later resumption
    ///
    /// Returns an [`IntermediateStorageRootState`] if the calculation needs to be resumed later, or
    /// `None` if the storage root was successfully computed and added to the trie.
    fn process_storage_root_result(
        &mut self,
        storage_result: StorageRootProgress,
        hashed_address: B256,
        account: Account,
        hash_builder: &mut HashBuilder,
        retain_updates: bool,
    ) -> Result<Option<IntermediateStorageRootState>, StateRootError> {
        match storage_result {
            StorageRootProgress::Complete(storage_root, storage_slots_walked, updates) => {
                // Storage root completed
                self.hashed_entries_walked += storage_slots_walked;
                if retain_updates {
                    self.updated_storage_nodes += updates.len();
                    self.trie_updates.insert_storage_updates(hashed_address, updates);
                }

                // Encode the account with the computed storage root
                self.account_rlp.clear();
                let trie_account = account.into_trie_account(storage_root);
                trie_account.encode(&mut self.account_rlp as &mut dyn BufMut);
                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &self.account_rlp);
                Ok(None)
            }
            StorageRootProgress::Progress(state, storage_slots_walked, updates) => {
                // Storage root hit threshold or resumed calculation hit threshold
                debug!(
                    target: "trie::state_root",
                    ?hashed_address,
                    storage_slots_walked,
                    last_storage_key = ?state.last_hashed_key,
                    ?account,
                    "Pausing storage root calculation"
                );

                self.hashed_entries_walked += storage_slots_walked;
                if retain_updates {
                    self.trie_updates.insert_storage_updates(hashed_address, updates);
                }

                Ok(Some(IntermediateStorageRootState { state: *state, account }))
            }
        }
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
    /// Previous intermediate state.
    previous_state: Option<IntermediateRootState>,
    /// The number of updates after which the intermediate progress should be returned.
    threshold: u64,
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
        prefix_set: PrefixSet,
        #[cfg(feature = "metrics")] metrics: TrieRootMetrics,
    ) -> Self {
        Self::new_hashed(
            trie_cursor_factory,
            hashed_cursor_factory,
            keccak256(address),
            prefix_set,
            #[cfg(feature = "metrics")]
            metrics,
        )
    }

    /// Creates a new storage root calculator given a hashed address.
    pub const fn new_hashed(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        hashed_address: B256,
        prefix_set: PrefixSet,
        #[cfg(feature = "metrics")] metrics: TrieRootMetrics,
    ) -> Self {
        Self {
            trie_cursor_factory,
            hashed_cursor_factory,
            hashed_address,
            prefix_set,
            previous_state: None,
            threshold: DEFAULT_INTERMEDIATE_THRESHOLD,
            #[cfg(feature = "metrics")]
            metrics,
        }
    }

    /// Set the changed prefixes.
    pub fn with_prefix_set(mut self, prefix_set: PrefixSet) -> Self {
        self.prefix_set = prefix_set;
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
    pub fn with_intermediate_state(mut self, state: Option<IntermediateRootState>) -> Self {
        self.previous_state = state;
        self
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> StorageRoot<T, HF> {
        StorageRoot {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            hashed_address: self.hashed_address,
            prefix_set: self.prefix_set,
            previous_state: self.previous_state,
            threshold: self.threshold,
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
            previous_state: self.previous_state,
            threshold: self.threshold,
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
    /// Walks the intermediate nodes of existing storage trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder. Collects the updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    pub fn root_with_progress(self) -> Result<StorageRootProgress, StorageRootError> {
        self.calculate(true)
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root and storage trie updates for a given address.
    pub fn root_with_updates(self) -> Result<(B256, usize, StorageTrieUpdates), StorageRootError> {
        match self.with_no_threshold().calculate(true)? {
            StorageRootProgress::Complete(root, walked, updates) => Ok((root, walked, updates)),
            StorageRootProgress::Progress(..) => unreachable!(), // unreachable threshold
        }
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root.
    pub fn root(self) -> Result<B256, StorageRootError> {
        match self.calculate(false)? {
            StorageRootProgress::Complete(root, _, _) => Ok(root),
            StorageRootProgress::Progress(..) => unreachable!(), // update retenion is disabled
        }
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root, number of walked entries and trie updates
    /// for a given address if requested.
    #[instrument(skip_all, target = "trie::storage_root", name = "Storage trie", fields(hashed_address = ?self.hashed_address))]
    pub fn calculate(self, retain_updates: bool) -> Result<StorageRootProgress, StorageRootError> {
        trace!(target: "trie::storage_root", "calculating storage root");

        let mut hashed_storage_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(self.hashed_address)?;

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty()? {
            return Ok(StorageRootProgress::Complete(
                EMPTY_ROOT_HASH,
                0,
                StorageTrieUpdates::deleted(),
            ));
        }

        let mut tracker = TrieTracker::default();
        let mut trie_updates = StorageTrieUpdates::default();

        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(self.hashed_address)?;

        let (mut hash_builder, mut storage_node_iter) = match self.previous_state {
            Some(state) => {
                let hash_builder = state.hash_builder.with_updates(retain_updates);
                let walker = TrieWalker::<_>::storage_trie_from_stack(
                    trie_cursor,
                    state.walker_stack,
                    self.prefix_set,
                )
                .with_deletions_retained(retain_updates);
                let node_iter = TrieNodeIter::storage_trie(walker, hashed_storage_cursor)
                    .with_last_hashed_key(state.last_hashed_key);
                (hash_builder, node_iter)
            }
            None => {
                let hash_builder = HashBuilder::default().with_updates(retain_updates);
                let walker = TrieWalker::storage_trie(trie_cursor, self.prefix_set)
                    .with_deletions_retained(retain_updates);
                let node_iter = TrieNodeIter::storage_trie(walker, hashed_storage_cursor);
                (hash_builder, node_iter)
            }
        };

        let mut hashed_entries_walked = 0;
        while let Some(node) = storage_node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    tracker.inc_branch();
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_slot, value) => {
                    tracker.inc_leaf();
                    hashed_entries_walked += 1;
                    hash_builder.add_leaf(
                        Nibbles::unpack(hashed_slot),
                        alloy_rlp::encode_fixed_size(&value).as_ref(),
                    );

                    // Check if we need to return intermediate progress
                    let total_updates_len =
                        storage_node_iter.walker.removed_keys_len() + hash_builder.updates_len();
                    if retain_updates && total_updates_len as u64 >= self.threshold {
                        let (walker_stack, walker_deleted_keys) = storage_node_iter.walker.split();
                        trie_updates.removed_nodes.extend(walker_deleted_keys);
                        let (hash_builder, hash_builder_updates) = hash_builder.split();
                        trie_updates.storage_nodes.extend(hash_builder_updates);

                        let state = IntermediateRootState {
                            hash_builder,
                            walker_stack,
                            last_hashed_key: hashed_slot,
                        };

                        return Ok(StorageRootProgress::Progress(
                            Box::new(state),
                            hashed_entries_walked,
                            trie_updates,
                        ));
                    }
                }
            }
        }

        let root = hash_builder.root();

        let removed_keys = storage_node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder, removed_keys);

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
        Ok(StorageRootProgress::Complete(root, storage_slots_walked, trie_updates))
    }
}

/// Trie type for differentiating between various trie calculations.
#[derive(Clone, Copy, Debug)]
pub enum TrieType {
    /// State trie type.
    State,
    /// Storage trie type.
    Storage,
}

impl TrieType {
    #[cfg(feature = "metrics")]
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Storage => "storage",
        }
    }
}
