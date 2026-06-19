#[cfg(feature = "trie-debug")]
use crate::debug_recorder::TrieDebugRecorder;
use crate::{
    lfu::BucketedLfu, traits::SparseTrie as SparseTrieTrait, ArenaParallelSparseTrie,
    RevealableSparseTrie,
};
use alloc::vec::Vec;
use alloy_primitives::{map::B256Map, B256};
use either::Either;
use reth_execution_errors::{SparseStateTrieResult, SparseTrieErrorKind};
#[cfg(feature = "std")]
use reth_trie_common::HashedPostStateSorted;
use reth_trie_common::{
    updates::{StorageTrieUpdates, TrieUpdates},
    DecodedMultiProof, MultiProof, Nibbles, ProofTrieNodeV2,
};
#[cfg(feature = "std")]
use tracing::debug;
use tracing::instrument;

/// Holds data that should be dropped after any locks are released.
///
/// This is used to defer expensive deallocations (like proof node buffers) until after final state
/// root is calculated
#[derive(Debug, Default)]
pub struct DeferredDrops {
    /// Each nodes reveal operation creates a new buffer, uses it, and pushes it here.
    pub proof_nodes_bufs: Vec<Vec<ProofTrieNodeV2>>,
}

#[derive(Debug)]
/// Sparse state trie representing lazy-loaded Ethereum state trie.
pub struct SparseStateTrie<
    A = ArenaParallelSparseTrie, // Account trie implementation
    S = ArenaParallelSparseTrie, // Storage trie implementation
> {
    /// Sparse account trie.
    state: RevealableSparseTrie<A>,
    /// State related to storage tries.
    storage: StorageTries<S>,
    /// Flag indicating whether trie updates should be retained.
    retain_updates: bool,
    /// Holds data that should be dropped after final state root is calculated.
    deferred_drops: DeferredDrops,
    /// Global LFU tracker for hot `(address, slot)` storage entries.
    hot_slots_lfu: BucketedLfu<HotSlotKey>,
    /// Global LFU tracker for hot account entries.
    hot_accounts_lfu: BucketedLfu<B256>,
    /// Metrics for the sparse state trie.
    #[cfg(feature = "metrics")]
    metrics: crate::metrics::SparseStateTrieMetrics,
}

impl<A, S> Default for SparseStateTrie<A, S>
where
    A: Default,
    S: Default,
{
    fn default() -> Self {
        Self {
            state: Default::default(),
            storage: Default::default(),
            retain_updates: false,
            deferred_drops: DeferredDrops::default(),
            hot_slots_lfu: BucketedLfu::default(),
            hot_accounts_lfu: BucketedLfu::default(),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }
}

#[cfg(test)]
impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: RevealableSparseTrie) -> Self {
        Self { state, ..Default::default() }
    }
}

impl<A, S> SparseStateTrie<A, S> {
    /// Set the retention of branch node updates and deletions.
    pub const fn set_updates(&mut self, retain_updates: bool) {
        self.retain_updates = retain_updates;
    }

    /// Set the retention of branch node updates and deletions.
    pub const fn with_updates(mut self, retain_updates: bool) -> Self {
        self.set_updates(retain_updates);
        self
    }

    /// Seeds the hot account/storage LFU caches with their configured capacities.
    ///
    /// This must happen before the first `record_*_touch` call, otherwise touches are ignored while
    /// the LFUs still have zero capacity.
    pub fn set_hot_cache_capacities(&mut self, max_hot_slots: usize, max_hot_accounts: usize) {
        self.hot_slots_lfu.decay_and_evict(max_hot_slots);
        self.hot_accounts_lfu.decay_and_evict(max_hot_accounts);
    }

    /// Seeds the hot account/storage LFU caches with their configured capacities.
    pub fn with_hot_cache_capacities(
        mut self,
        max_hot_slots: usize,
        max_hot_accounts: usize,
    ) -> Self {
        self.set_hot_cache_capacities(max_hot_slots, max_hot_accounts);
        self
    }

    /// Set the accounts trie to the given `RevealableSparseTrie`.
    pub fn set_accounts_trie(&mut self, trie: RevealableSparseTrie<A>) {
        self.state = trie;
    }

    /// Set the accounts trie to the given `RevealableSparseTrie`.
    pub fn with_accounts_trie(mut self, trie: RevealableSparseTrie<A>) -> Self {
        self.set_accounts_trie(trie);
        self
    }

    /// Set the default trie which will be cloned when creating new storage
    /// [`RevealableSparseTrie`]s.
    pub fn set_default_storage_trie(&mut self, trie: RevealableSparseTrie<S>) {
        self.storage.default_trie = trie;
    }

    /// Set the default trie which will be cloned when creating new storage
    /// [`RevealableSparseTrie`]s.
    pub fn with_default_storage_trie(mut self, trie: RevealableSparseTrie<S>) -> Self {
        self.set_default_storage_trie(trie);
        self
    }

    /// Takes the data structures for deferred dropping.
    ///
    /// This allows the caller to drop the buffers later, avoiding expensive deallocations while
    /// calculating the state root.
    pub fn take_deferred_drops(&mut self) -> DeferredDrops {
        core::mem::take(&mut self.deferred_drops)
    }
}

impl SparseStateTrie {
    /// Create new [`SparseStateTrie`] with the default trie implementation.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<A: SparseTrieTrait, S: SparseTrieTrait> SparseStateTrie<A, S> {
    /// Takes all debug recorders from the account trie and all revealed storage tries.
    ///
    /// Returns a vec of `(Option<B256>, TrieDebugRecorder)` where `None` is the account trie
    /// key, and `Some(address)` are storage trie keys.
    #[cfg(feature = "trie-debug")]
    pub fn take_debug_recorders(&mut self) -> alloc::vec::Vec<(Option<B256>, TrieDebugRecorder)> {
        let mut recorders = alloc::vec::Vec::new();
        if let Some(trie) = self.state.as_revealed_mut() {
            recorders.push((None, trie.take_debug_recorder()));
        }
        for (address, trie) in &mut self.storage.tries {
            if let Some(trie) = trie.as_revealed_mut() {
                recorders.push((Some(*address), trie.take_debug_recorder()));
            }
        }
        recorders
    }
}

impl<A, S> SparseStateTrie<A, S>
where
    A: SparseTrieTrait + Default,
    S: SparseTrieTrait + Default + Clone,
{
    /// Returns mutable reference to account trie.
    pub const fn trie_mut(&mut self) -> &mut RevealableSparseTrie<A> {
        &mut self.state
    }

    /// Returns `true` if the account path has been revealed in the sparse trie.
    pub fn is_account_revealed(&self, account: B256) -> bool {
        let path = Nibbles::unpack(account);
        let trie = match self.state_trie_ref() {
            Some(t) => t,
            None => return false,
        };

        trie.find_leaf(&path, None).is_ok()
    }

    /// Was the storage-slot witness for (`address`,`slot`) complete?
    pub fn check_valid_storage_witness(&self, address: B256, slot: B256) -> bool {
        let path = Nibbles::unpack(slot);
        let trie = match self.storage_trie_ref(&address) {
            Some(t) => t,
            None => return false,
        };

        trie.find_leaf(&path, None).is_ok()
    }

    /// Records a storage slot access/update in the global LFU tracker.
    #[inline]
    pub fn record_slot_touch(&mut self, account: B256, slot: B256) {
        self.hot_slots_lfu.touch(HotSlotKey { address: account, slot });
    }

    /// Records an account access/update in the global LFU tracker.
    #[inline]
    pub fn record_account_touch(&mut self, account: B256) {
        self.hot_accounts_lfu.touch(account);
    }

    /// Returns reference to bytes representing leaf value for the target account.
    pub fn get_account_value(&self, account: &B256) -> Option<&Vec<u8>> {
        self.state.as_revealed_ref()?.get_leaf_value(&Nibbles::unpack(account))
    }

    /// Returns reference to bytes representing leaf value for the target account and storage slot.
    pub fn get_storage_slot_value(&self, account: &B256, slot: &B256) -> Option<&Vec<u8>> {
        self.storage.tries.get(account)?.as_revealed_ref()?.get_leaf_value(&Nibbles::unpack(slot))
    }

    /// Returns reference to state trie if it was revealed.
    pub const fn state_trie_ref(&self) -> Option<&A> {
        self.state.as_revealed_ref()
    }

    /// Returns reference to storage trie if it was revealed.
    pub fn storage_trie_ref(&self, address: &B256) -> Option<&S> {
        self.storage.tries.get(address).and_then(|e| e.as_revealed_ref())
    }

    /// Returns mutable reference to storage sparse trie if it was revealed.
    pub fn storage_trie_mut(&mut self, address: &B256) -> Option<&mut S> {
        self.storage.tries.get_mut(address).and_then(|e| e.as_revealed_mut())
    }

    /// Returns mutable reference to storage tries.
    pub const fn storage_tries_mut(&mut self) -> &mut B256Map<RevealableSparseTrie<S>> {
        &mut self.storage.tries
    }

    /// Takes the storage trie for the provided address.
    pub fn take_storage_trie(&mut self, address: &B256) -> Option<RevealableSparseTrie<S>> {
        self.storage.tries.remove(address)
    }

    /// Takes the storage trie for the provided address, creating a blind one if it doesn't exist.
    pub fn take_or_create_storage_trie(&mut self, address: &B256) -> RevealableSparseTrie<S> {
        self.storage.tries.remove(address).unwrap_or_else(|| {
            self.storage.cleared_tries.pop().unwrap_or_else(|| self.storage.default_trie.clone())
        })
    }

    /// Inserts storage trie for the provided address.
    pub fn insert_storage_trie(&mut self, address: B256, storage_trie: RevealableSparseTrie<S>) {
        self.storage.tries.insert(address, storage_trie);
    }

    /// Returns mutable reference to storage sparse trie, creating a blind one if it doesn't exist.
    pub fn get_or_create_storage_trie_mut(
        &mut self,
        address: B256,
    ) -> &mut RevealableSparseTrie<S> {
        self.storage.get_or_create_trie_mut(address)
    }

    /// Reveal unknown trie paths from multiproof.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_multiproof(&mut self, multiproof: MultiProof) -> SparseStateTrieResult<()> {
        // first decode the multiproof
        let decoded_multiproof = multiproof.try_into()?;

        // then reveal the decoded multiproof
        self.reveal_decoded_multiproof(decoded_multiproof)
    }

    /// Reveal unknown trie paths from decoded multiproof.
    /// NOTE: This method does not extensively validate the proof.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn reveal_decoded_multiproof(
        &mut self,
        multiproof: DecodedMultiProof,
    ) -> SparseStateTrieResult<()> {
        self.reveal_decoded_multiproof_v2(multiproof.into())
    }

    /// Reveals a V2 decoded multiproof.
    ///
    /// V2 multiproofs use a simpler format where proof nodes are stored as vectors rather than
    /// hashmaps, with masks already included in the `ProofTrieNode` structure.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn reveal_decoded_multiproof_v2(
        &mut self,
        multiproof: reth_trie_common::DecodedMultiProofV2,
    ) -> SparseStateTrieResult<()> {
        let reth_trie_common::DecodedMultiProofV2 { account_proofs, mut storage_proofs, .. } =
            multiproof;

        // Collect `(trie, proof_nodes)` pairs for both the account trie and every storage trie
        // touched by this multiproof.
        let mut targets = Vec::with_capacity(storage_proofs.len() + 1);

        if !account_proofs.is_empty() {
            #[cfg(feature = "metrics")]
            self.metrics.increment_total_account_nodes(account_proofs.len() as u64);
            targets.push((Either::Left(&mut self.state), account_proofs));
        }

        // Ensure a storage trie exists for every address whose proofs we're about to reveal
        for &account in storage_proofs.keys() {
            let _ = self.storage.get_or_create_trie_mut(account);
        }

        for (account, trie) in &mut self.storage.tries {
            if let Some(nodes) = storage_proofs.remove(account) {
                #[cfg(feature = "metrics")]
                self.metrics.increment_total_storage_nodes(nodes.len() as u64);
                targets.push((Either::Right(trie), nodes));
            }
        }

        let retain_updates = self.retain_updates;

        #[cfg(not(feature = "std"))]
        let results: Vec<_> = targets
            .into_iter()
            .map(|(target, mut nodes)| {
                let result = match target {
                    Either::Left(trie) => trie.reveal_v2_proof_nodes(&mut nodes, retain_updates),
                    Either::Right(trie) => trie.reveal_v2_proof_nodes(&mut nodes, retain_updates),
                };
                (result, nodes)
            })
            .collect();

        #[cfg(feature = "std")]
        let results: Vec<_> = {
            use rayon::iter::ParallelIterator;
            use reth_primitives_traits::ParallelBridgeBuffered;

            targets
                .into_iter()
                .par_bridge_buffered()
                .map(|(target, mut nodes)| {
                    let result = match target {
                        Either::Left(trie) => {
                            trie.reveal_v2_proof_nodes(&mut nodes, retain_updates)
                        }
                        Either::Right(trie) => {
                            trie.reveal_v2_proof_nodes(&mut nodes, retain_updates)
                        }
                    };
                    (result, nodes)
                })
                .collect()
        };

        // Accumulate the first error and defer dropping the proof node buffers.
        let mut any_err = Ok(());
        for (result, nodes) in results {
            if result.is_err() && any_err.is_ok() {
                any_err = result.map_err(Into::into);
            }
            self.deferred_drops.proof_nodes_bufs.push(nodes);
        }

        any_err
    }

    /// Wipe the storage trie at the provided address.
    pub fn wipe_storage(&mut self, address: B256) -> SparseStateTrieResult<()> {
        if let Some(trie) = self.storage.tries.get_mut(&address) {
            trie.wipe()?;
        }
        Ok(())
    }

    /// Calculates the hashes of subtries.
    ///
    /// If the trie has not been revealed, this function does nothing.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn calculate_subtries(&mut self) {
        if let RevealableSparseTrie::Revealed(trie) = &mut self.state {
            trie.update_subtrie_hashes();
        }
    }

    /// Returns storage sparse trie root if the trie has been revealed.
    pub fn storage_root(&mut self, account: &B256) -> Option<B256> {
        self.storage.tries.get_mut(account).and_then(|trie| trie.root())
    }

    /// Returns mutable reference to the revealed account sparse trie.
    fn revealed_trie_mut(&mut self) -> SparseStateTrieResult<&mut A> {
        self.state.as_revealed_mut().ok_or_else(|| SparseTrieErrorKind::Blind.into())
    }

    /// Returns sparse trie root.
    pub fn root(&mut self) -> SparseStateTrieResult<B256> {
        // record revealed node metrics
        #[cfg(feature = "metrics")]
        self.metrics.record();

        Ok(self.revealed_trie_mut()?.root())
    }

    /// Returns sparse trie root and trie updates.
    ///
    /// Returns an error if the account trie is still blind.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn root_with_updates(&mut self) -> SparseStateTrieResult<(B256, TrieUpdates)> {
        // record revealed node metrics
        #[cfg(feature = "metrics")]
        self.metrics.record();

        let storage_tries = self.storage_trie_updates();
        let revealed = self.revealed_trie_mut()?;

        let (root, updates) = (revealed.root(), revealed.take_updates());
        let updates = TrieUpdates {
            account_nodes: updates.updated_nodes,
            removed_nodes: updates.removed_nodes,
            storage_tries,
        };
        Ok((root, updates))
    }

    /// Returns storage trie updates for tries that have been revealed.
    ///
    /// Panics if any of the storage tries are not revealed.
    pub fn storage_trie_updates(&mut self) -> B256Map<StorageTrieUpdates> {
        self.storage
            .tries
            .iter_mut()
            .map(|(address, trie)| {
                let trie = trie.as_revealed_mut().unwrap();
                let updates = trie.take_updates();
                let updates = StorageTrieUpdates {
                    is_deleted: updates.wiped,
                    storage_nodes: updates.updated_nodes,
                    removed_nodes: updates.removed_nodes,
                };
                (*address, updates)
            })
            .filter(|(_, updates)| !updates.is_empty())
            .collect()
    }

    /// Returns [`TrieUpdates`] by taking the updates from the revealed sparse tries.
    ///
    /// Returns `None` if the accounts trie is not revealed.
    pub fn take_trie_updates(&mut self) -> Option<TrieUpdates> {
        let storage_tries = self.storage_trie_updates();
        self.state.as_revealed_mut().map(|state| {
            let updates = state.take_updates();
            TrieUpdates {
                account_nodes: updates.updated_nodes,
                removed_nodes: updates.removed_nodes,
                storage_tries,
            }
        })
    }
}

impl<A, S> SparseStateTrie<A, S>
where
    A: SparseTrieTrait + Default,
    S: SparseTrieTrait + Default + Clone,
{
    /// Clears all trie data while preserving allocations for reuse.
    ///
    /// This resets the trie to an empty state but keeps the underlying memory allocations,
    /// which can significantly reduce allocation overhead when the trie is reused.
    pub fn clear(&mut self) {
        self.state.clear();
        self.storage.clear();
    }

    /// Returns a heuristic for the total in-memory size of this state trie in bytes.
    ///
    /// This aggregates the memory usage of the account trie, all revealed storage tries
    /// (including cleared ones retained for allocation reuse), and auxiliary data structures.
    pub fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();

        size += match &self.state {
            RevealableSparseTrie::Revealed(t) | RevealableSparseTrie::Blind(Some(t)) => {
                t.memory_size()
            }
            RevealableSparseTrie::Blind(None) => 0,
        };

        for trie in self.storage.tries.values() {
            size += match trie {
                RevealableSparseTrie::Revealed(t) | RevealableSparseTrie::Blind(Some(t)) => {
                    t.memory_size()
                }
                RevealableSparseTrie::Blind(None) => 0,
            };
        }
        for trie in &self.storage.cleared_tries {
            size += match trie {
                RevealableSparseTrie::Revealed(t) | RevealableSparseTrie::Blind(Some(t)) => {
                    t.memory_size()
                }
                RevealableSparseTrie::Blind(None) => 0,
            };
        }

        size
    }

    /// Returns the number of storage tries currently retained (active + cleared).
    pub fn retained_storage_tries_count(&self) -> usize {
        self.storage.tries.len() + self.storage.cleared_tries.len()
    }

    /// Prunes account/storage tries according to global LFU retention and retained paths.
    ///
    /// - Top LFU `(address, slot)` entries are retained up to `max_hot_slots`.
    ///
    /// - Top LFU `(address, slot)` entries are retained in storage tries.
    /// - Account trie retains only paths for accounts tracked by the account LFU.
    /// - Storage tries retain only paths needed for retained slots.
    /// - Additional retained paths are unioned with LFU-selected paths.
    /// - All other revealed paths are pruned to hash stubs or fully evicted.
    ///
    /// # Preconditions
    ///
    /// All revealed account and storage tries must already have computed hashes via `root()`
    /// / `storage_root()` for their current state. Pruning a dirty revealed trie is a hard
    /// error and may panic.
    #[cfg(feature = "std")]
    #[instrument(
        level = "debug",
        name = "SparseStateTrie::prune",
        target = "trie::sparse",
        skip_all,
        fields(%max_hot_slots, %max_hot_accounts)
    )]
    pub fn prune(
        &mut self,
        max_hot_slots: usize,
        max_hot_accounts: usize,
        mut retained_paths: SparseTrieRetainedPaths,
    ) {
        self.hot_slots_lfu.decay_and_evict(max_hot_slots);
        self.hot_accounts_lfu.decay_and_evict(max_hot_accounts);
        retained_paths.extend_from_lfus(&self.hot_accounts_lfu, &self.hot_slots_lfu);
        retained_paths.sort_and_dedup();

        let retained_accounts = retained_paths.account_paths.len();
        let retained_storage_tries = retained_paths.storage_slots.len();
        let total_storage_tries_before = self.storage.tries.len();

        let SparseTrieRetainedPaths { account_paths, storage_slots } = retained_paths;

        // Prune account and storage tries in parallel using the same retained set.
        let (account_nodes_pruned, storage_tries_evicted) = rayon::join(
            || self.state.as_revealed_mut().map(|trie| trie.prune(&account_paths)).unwrap_or(0),
            || self.storage.prune_by_retained_slots(storage_slots),
        );

        debug!(
            target: "trie::sparse",
            retained_accounts,
            retained_storage_tries,
            account_nodes_pruned,
            storage_tries_evicted,
            storage_tries_after = total_storage_tries_before - storage_tries_evicted,
            "SparseStateTrie::prune completed"
        );
    }
}

/// Account and storage paths that should survive sparse trie pruning.
#[derive(Debug, Default)]
#[cfg(feature = "std")]
pub struct SparseTrieRetainedPaths {
    account_paths: Vec<Nibbles>,
    storage_slots: B256Map<Vec<Nibbles>>,
}

#[cfg(feature = "std")]
impl SparseTrieRetainedPaths {
    /// Retains the account path for the given hashed address.
    pub fn retain_account(&mut self, hashed_address: B256) {
        self.account_paths.push(Nibbles::unpack(hashed_address));
    }

    /// Retains the storage slot paths for the given hashed address.
    pub fn retain_storage_slots(
        &mut self,
        hashed_address: B256,
        slots: impl IntoIterator<Item = Nibbles>,
    ) {
        self.storage_slots.entry(hashed_address).or_default().extend(slots);
    }

    /// Extends the retained paths with every account and storage slot touched by the hashed state.
    pub fn extend_from_hashed_state(&mut self, hashed_state: &HashedPostStateSorted) {
        self.account_paths
            .extend(hashed_state.accounts().iter().map(|(address, _)| Nibbles::unpack(*address)));

        for (address, storage) in hashed_state.account_storages() {
            self.retain_account(*address);
            self.retain_storage_slots(
                *address,
                storage.storage_slots_ref().iter().map(|(slot, _)| Nibbles::unpack(*slot)),
            );
        }
    }

    fn extend_from_lfus(
        &mut self,
        hot_accounts: &BucketedLfu<B256>,
        hot_slots: &BucketedLfu<HotSlotKey>,
    ) {
        self.account_paths.extend(hot_accounts.keys().map(|key| Nibbles::unpack(*key)));
        for key in hot_slots.keys() {
            self.storage_slots.entry(key.address).or_default().push(Nibbles::unpack(key.slot));
        }
    }

    fn sort_and_dedup(&mut self) {
        self.account_paths.sort_unstable();
        self.account_paths.dedup();

        self.storage_slots.retain(|_, slots| {
            slots.sort_unstable();
            slots.dedup();
            !slots.is_empty()
        });
    }
}

/// The fields of [`SparseStateTrie`] related to storage tries. This is kept separate from the rest
/// of [`SparseStateTrie`] to help enforce allocation re-use.
#[derive(Debug, Default)]
struct StorageTries<S = ArenaParallelSparseTrie> {
    /// Sparse storage tries.
    tries: B256Map<RevealableSparseTrie<S>>,
    /// Cleared storage tries, kept for re-use.
    cleared_tries: Vec<RevealableSparseTrie<S>>,
    /// A default cleared trie instance, which will be cloned when creating new tries.
    default_trie: RevealableSparseTrie<S>,
}

#[cfg(feature = "std")]
impl<S: SparseTrieTrait> StorageTries<S> {
    /// Prunes storage tries using LFU-retained slots.
    ///
    /// Tries without retained slots are evicted entirely. Tries with retained slots are pruned to
    /// those slots.
    fn prune_by_retained_slots(&mut self, retained_slots: B256Map<Vec<Nibbles>>) -> usize {
        // Parallel pass: prune retained tries and clear evicted ones in place.
        {
            use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
            self.tries.par_iter_mut().for_each(|(address, trie)| {
                if let Some(slots) = retained_slots.get(address) {
                    if let Some(t) = trie.as_revealed_mut() {
                        t.prune(slots);
                    }
                } else {
                    trie.clear();
                }
            });
        }

        // Cheap sequential drain: move already-cleared tries into the reuse pool.
        let addresses_to_evict: Vec<B256> = self
            .tries
            .keys()
            .filter(|address| !retained_slots.contains_key(*address))
            .copied()
            .collect();

        let evicted = addresses_to_evict.len();
        self.cleared_tries.reserve(evicted);
        for address in &addresses_to_evict {
            if let Some(trie) = self.tries.remove(address) {
                self.cleared_tries.push(trie);
            }
        }

        evicted
    }
}

impl<S: SparseTrieTrait> StorageTries<S> {
    /// Returns all fields to a cleared state, equivalent to the default state, keeping cleared
    /// collections for re-use later when possible.
    fn clear(&mut self) {
        self.cleared_tries.extend(self.tries.drain().map(|(_, mut trie)| {
            trie.clear();
            trie
        }));
    }
}

impl<S: SparseTrieTrait + Clone> StorageTries<S> {
    // Returns mutable reference to storage sparse trie, creating a blind one if it doesn't exist.
    fn get_or_create_trie_mut(&mut self, address: B256) -> &mut RevealableSparseTrie<S> {
        self.tries.entry(address).or_insert_with(|| {
            self.cleared_tries.pop().unwrap_or_else(|| self.default_trie.clone())
        })
    }
}

/// Key for identifying a storage slot in the global LFU cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HotSlotKey {
    address: B256,
    slot: B256,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArenaParallelSparseTrie, LeafLookup, LeafUpdate};
    use alloy_primitives::{
        b256,
        map::{HashMap, HashSet},
        U256,
    };
    use arbitrary::Arbitrary;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_execution_errors::{SparseStateTrieErrorKind, SparseTrieErrorKind};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::StorageTrieUpdates, HashBuilder, MultiProof, EMPTY_ROOT_HASH};
    use reth_trie_common::{
        proof::{ProofNodes, ProofRetainer},
        BranchNodeMasks, BranchNodeMasksMap, BranchNodeV2, HashedStorageSorted, LeafNode, RlpNode,
        StorageMultiProof, TrieAccount, TrieMask, TrieNodeV2,
    };

    /// Create a leaf key (suffix) with given nibbles padded with zeros to reach `total_len`.
    fn leaf_key(suffix: impl AsRef<[u8]>, total_len: usize) -> Nibbles {
        let suffix = suffix.as_ref();
        let mut nibbles = Nibbles::from_nibbles(suffix);
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![0; total_len - suffix.len()]));
        nibbles
    }

    fn apply_account_update(sparse: &mut SparseStateTrie, address: B256, update: LeafUpdate) {
        let mut updates = B256Map::from_iter([(address, update)]);
        sparse.trie_mut().update_leaves(&mut updates, |_, _| {}).unwrap();
        assert!(updates.is_empty());
    }

    #[test]
    fn reveal_account_path_twice() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        // Full 64-nibble paths
        let full_path_0 = leaf_key([0x0], 64);
        let _full_path_1 = leaf_key([0x1], 64);

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        // Leaf key is 63 nibbles (suffix after 1-nibble node path)
        let leaf_1 = alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            leaf_value.clone(),
        )));
        let leaf_2 = alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            leaf_value.clone(),
        )));

        let multiproof = MultiProof {
            account_subtree: ProofNodes::from_iter([
                (
                    Nibbles::default(),
                    alloy_rlp::encode(TrieNodeV2::Branch(BranchNodeV2 {
                        key: Nibbles::default(),
                        stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                        state_mask: TrieMask::new(0b11),
                        branch_rlp_node: None,
                    }))
                    .into(),
                ),
                (Nibbles::from_nibbles([0x0]), leaf_1.clone().into()),
                (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
            ]),
            ..Default::default()
        };

        // Reveal multiproof and check that the state trie contains the leaf node and value
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::Exists)
        ));
        assert_eq!(
            sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0),
            Some(&leaf_value)
        );

        // Remove the leaf node and check that the state trie does not contain the leaf node and
        // value
        apply_account_update(&mut sparse, B256::ZERO, LeafUpdate::Changed(Vec::new()));
        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::NonExistent)
        ));
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());
    }

    #[test]
    fn reveal_storage_path_twice() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        // Full 64-nibble path
        let full_path_0 = leaf_key([0x0], 64);

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        let leaf_1 = alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            leaf_value.clone(),
        )));
        let leaf_2 = alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            leaf_value.clone(),
        )));

        let multiproof = MultiProof {
            storages: HashMap::from_iter([(
                B256::ZERO,
                StorageMultiProof {
                    root: B256::ZERO,
                    subtree: ProofNodes::from_iter([
                        (
                            Nibbles::default(),
                            alloy_rlp::encode(TrieNodeV2::Branch(BranchNodeV2 {
                                key: Nibbles::default(),
                                stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                                state_mask: TrieMask::new(0b11),
                                branch_rlp_node: None,
                            }))
                            .into(),
                        ),
                        (Nibbles::from_nibbles([0x0]), leaf_1.clone().into()),
                        (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
                    ]),
                    branch_node_masks: Default::default(),
                },
            )]),
            ..Default::default()
        };

        // Reveal multiproof and check that the storage trie contains the leaf node and value
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        assert!(matches!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::Exists)
        ));
        assert_eq!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().get_leaf_value(&full_path_0),
            Some(&leaf_value)
        );

        // Remove the leaf node and check that the storage trie does not contain the leaf node and
        // value
        let mut updates = B256Map::from_iter([(B256::ZERO, LeafUpdate::Changed(Vec::new()))]);
        sparse
            .storage_trie_mut(&B256::ZERO)
            .unwrap()
            .update_leaves(&mut updates, |_, _| {})
            .unwrap();
        assert!(updates.is_empty());
        assert!(matches!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::NonExistent)
        ));
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&full_path_0)
            .is_none());
    }

    #[test]
    fn seeded_hot_cache_capacities_preserve_first_cycle_touches() {
        let account = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
        let slot = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        sparse.set_hot_cache_capacities(1, 1);
        sparse.record_account_touch(account);
        sparse.record_slot_touch(account, slot);
        sparse.prune(1, 1, SparseTrieRetainedPaths::default());

        assert_eq!(sparse.hot_accounts_lfu.keys().copied().collect::<Vec<_>>(), vec![account]);
        assert_eq!(
            sparse.hot_slots_lfu.keys().copied().collect::<Vec<_>>(),
            vec![HotSlotKey { address: account, slot }]
        );
    }

    #[test]
    fn retained_paths_extend_from_hashed_state() {
        let account = B256::with_last_byte(0x01);
        let storage_account = B256::with_last_byte(0x02);
        let slot = B256::with_last_byte(0x03);
        let hashed_state = HashedPostStateSorted::new(
            vec![(account, Some(Account::default()))],
            B256Map::from_iter([(
                storage_account,
                HashedStorageSorted { storage_slots: vec![(slot, U256::from(1))], wiped: false },
            )]),
        );

        let mut retained_paths = SparseTrieRetainedPaths::default();
        retained_paths.extend_from_hashed_state(&hashed_state);
        retained_paths.sort_and_dedup();

        assert_eq!(
            retained_paths.account_paths,
            vec![Nibbles::unpack(account), Nibbles::unpack(storage_account)]
        );
        assert_eq!(retained_paths.storage_slots[&storage_account], vec![Nibbles::unpack(slot)]);
    }

    #[test]
    fn prune_keeps_retained_paths_overlay_account_and_storage() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        let account = B256::ZERO;
        let slot = B256::ZERO;
        let account_path = leaf_key([0x0], 64);
        let storage_path = leaf_key([0x0], 64);

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        let leaf_0 = alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            leaf_value.clone(),
        )));
        let leaf_1 =
            alloy_rlp::encode(TrieNodeV2::Leaf(LeafNode::new(leaf_key([], 63), leaf_value)));

        let subtree = || {
            ProofNodes::from_iter([
                (
                    Nibbles::default(),
                    alloy_rlp::encode(TrieNodeV2::Branch(BranchNodeV2 {
                        key: Nibbles::default(),
                        stack: vec![RlpNode::from_rlp(&leaf_0), RlpNode::from_rlp(&leaf_1)],
                        state_mask: TrieMask::new(0b11),
                        branch_rlp_node: None,
                    }))
                    .into(),
                ),
                (Nibbles::from_nibbles([0x0]), leaf_0.clone().into()),
                (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
            ])
        };

        let multiproof = MultiProof {
            account_subtree: subtree(),
            storages: HashMap::from_iter([(
                account,
                StorageMultiProof {
                    root: B256::ZERO,
                    subtree: subtree(),
                    branch_node_masks: Default::default(),
                },
            )]),
            ..Default::default()
        };

        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        let mut trie_account = TrieAccount::default();
        trie_account.storage_root = sparse.storage_root(&account).unwrap();
        apply_account_update(
            &mut sparse,
            account,
            LeafUpdate::Changed(alloy_rlp::encode(trie_account)),
        );
        sparse.root().unwrap();

        let mut retained_paths = SparseTrieRetainedPaths::default();
        retained_paths.retain_account(account);
        retained_paths.retain_storage_slots(account, [Nibbles::unpack(slot)]);
        sparse.prune(0, 0, retained_paths);

        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&account_path, None),
            Ok(LeafLookup::Exists)
        ));
        assert!(matches!(
            sparse.storage_trie_ref(&account).unwrap().find_leaf(&storage_path, None),
            Ok(LeafLookup::Exists)
        ));
    }

    #[test]
    fn reveal_v2_proof_nodes() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        // Full 64-nibble path
        let full_path_0 = leaf_key([0x0], 64);

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        let leaf_1_node = TrieNodeV2::Leaf(LeafNode::new(leaf_key([], 63), leaf_value.clone()));
        let leaf_2_node = TrieNodeV2::Leaf(LeafNode::new(leaf_key([], 63), leaf_value.clone()));

        let branch_node = TrieNodeV2::Branch(BranchNodeV2 {
            key: Nibbles::default(),
            stack: vec![
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1_node)),
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2_node)),
            ],
            state_mask: TrieMask::new(0b11),
            branch_rlp_node: None,
        });

        // Create V2 proof nodes with masks already included
        let v2_proof_nodes = vec![
            ProofTrieNodeV2 {
                path: Nibbles::default(),
                node: branch_node,
                masks: Some(BranchNodeMasks {
                    hash_mask: TrieMask::default(),
                    tree_mask: TrieMask::default(),
                }),
            },
            ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x0]), node: leaf_1_node, masks: None },
            ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x1]), node: leaf_2_node, masks: None },
        ];

        // Reveal V2 proof nodes
        sparse
            .reveal_decoded_multiproof_v2(reth_trie_common::DecodedMultiProofV2 {
                account_proofs: v2_proof_nodes,
                ..Default::default()
            })
            .unwrap();

        // Check that the state trie contains the leaf node and value
        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::Exists)
        ));
        assert_eq!(
            sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0),
            Some(&leaf_value)
        );

        // Remove the leaf node
        apply_account_update(&mut sparse, B256::ZERO, LeafUpdate::Changed(Vec::new()));
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());
    }

    #[test]
    fn reveal_storage_v2_proof_nodes() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        // Full 64-nibble path
        let full_path_0 = leaf_key([0x0], 64);

        let storage_value: Vec<u8> = alloy_rlp::encode_fixed_size(&U256::from(42)).to_vec();
        let leaf_1_node = TrieNodeV2::Leaf(LeafNode::new(leaf_key([], 63), storage_value.clone()));
        let leaf_2_node = TrieNodeV2::Leaf(LeafNode::new(leaf_key([], 63), storage_value.clone()));

        let branch_node = TrieNodeV2::Branch(BranchNodeV2 {
            key: Nibbles::default(),
            stack: vec![
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1_node)),
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2_node)),
            ],
            state_mask: TrieMask::new(0b11),
            branch_rlp_node: None,
        });

        let v2_proof_nodes = vec![
            ProofTrieNodeV2 { path: Nibbles::default(), node: branch_node, masks: None },
            ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x0]), node: leaf_1_node, masks: None },
            ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x1]), node: leaf_2_node, masks: None },
        ];

        // Reveal V2 storage proof nodes for account
        sparse
            .reveal_decoded_multiproof_v2(reth_trie_common::DecodedMultiProofV2 {
                storage_proofs: B256Map::from_iter([(B256::ZERO, v2_proof_nodes)]),
                ..Default::default()
            })
            .unwrap();

        // Check that the storage trie contains the leaf node and value
        assert!(matches!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::Exists)
        ));
        assert_eq!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().get_leaf_value(&full_path_0),
            Some(&storage_value)
        );

        // Remove the leaf node
        let mut updates = B256Map::from_iter([(B256::ZERO, LeafUpdate::Changed(Vec::new()))]);
        sparse
            .storage_trie_mut(&B256::ZERO)
            .unwrap()
            .update_leaves(&mut updates, |_, _| {})
            .unwrap();
        assert!(updates.is_empty());
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&full_path_0)
            .is_none());
    }

    #[test]
    fn root_on_blind_trie_returns_blind_error() {
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default();

        let err = sparse.root().unwrap_err();

        assert!(matches!(err.kind(), SparseStateTrieErrorKind::Sparse(SparseTrieErrorKind::Blind)));
    }

    #[test]
    fn take_trie_updates() {
        reth_tracing::init_test_tracing();

        // let mut rng = generators::rng();
        let mut rng = StdRng::seed_from_u64(1);

        let mut bytes = [0u8; 1024];
        rng.fill(bytes.as_mut_slice());

        let slot_1 = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
        let slot_path_1 = Nibbles::unpack(slot_1);
        let value_1 = U256::from(rng.random::<u64>());
        let slot_2 = b256!("0x1100000000000000000000000000000000000000000000000000000000000000");
        let slot_path_2 = Nibbles::unpack(slot_2);
        let value_2 = U256::from(rng.random::<u64>());
        let slot_3 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
        let value_3 = U256::from(rng.random::<u64>());

        let mut storage_hash_builder = HashBuilder::default()
            .with_proof_retainer(ProofRetainer::from_iter([slot_path_1, slot_path_2]));
        storage_hash_builder.add_leaf(slot_path_1, &alloy_rlp::encode_fixed_size(&value_1));
        storage_hash_builder.add_leaf(slot_path_2, &alloy_rlp::encode_fixed_size(&value_2));

        let storage_root = storage_hash_builder.root();
        let storage_proof_nodes = storage_hash_builder.take_proof_nodes();
        let storage_branch_node_masks = BranchNodeMasksMap::from_iter([
            (
                Nibbles::default(),
                BranchNodeMasks { hash_mask: TrieMask::new(0b010), tree_mask: TrieMask::default() },
            ),
            (
                Nibbles::from_nibbles([0x1]),
                BranchNodeMasks { hash_mask: TrieMask::new(0b11), tree_mask: TrieMask::default() },
            ),
        ]);

        let address_1 = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
        let address_path_1 = Nibbles::unpack(address_1);
        let account_1 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
        let mut trie_account_1 = account_1.into_trie_account(storage_root);
        let address_2 = b256!("0x1100000000000000000000000000000000000000000000000000000000000000");
        let address_path_2 = Nibbles::unpack(address_2);
        let account_2 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
        let mut trie_account_2 = account_2.into_trie_account(EMPTY_ROOT_HASH);

        let mut hash_builder = HashBuilder::default()
            .with_proof_retainer(ProofRetainer::from_iter([address_path_1, address_path_2]));
        hash_builder.add_leaf(address_path_1, &alloy_rlp::encode(trie_account_1));
        hash_builder.add_leaf(address_path_2, &alloy_rlp::encode(trie_account_2));

        let root = hash_builder.root();
        let proof_nodes = hash_builder.take_proof_nodes();
        let mut sparse = SparseStateTrie::<ArenaParallelSparseTrie>::default().with_updates(true);
        sparse
            .reveal_decoded_multiproof(
                MultiProof {
                    account_subtree: proof_nodes,
                    branch_node_masks: BranchNodeMasksMap::from_iter([(
                        Nibbles::from_nibbles([0x1]),
                        BranchNodeMasks {
                            hash_mask: TrieMask::new(0b00),
                            tree_mask: TrieMask::default(),
                        },
                    )]),
                    storages: HashMap::from_iter([
                        (
                            address_1,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes.clone(),
                                branch_node_masks: storage_branch_node_masks.clone(),
                            },
                        ),
                        (
                            address_2,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes,
                                branch_node_masks: storage_branch_node_masks,
                            },
                        ),
                    ]),
                }
                .try_into()
                .unwrap(),
            )
            .unwrap();

        assert_eq!(sparse.root().unwrap(), root);

        let address_3 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
        let account_3 = Account { nonce: account_1.nonce + 1, ..account_1 };
        let trie_account_3 = account_3.into_trie_account(EMPTY_ROOT_HASH);

        apply_account_update(
            &mut sparse,
            address_3,
            LeafUpdate::Changed(alloy_rlp::encode(trie_account_3)),
        );

        let mut updates =
            B256Map::from_iter([(slot_3, LeafUpdate::Changed(alloy_rlp::encode(value_3)))]);
        sparse
            .storage_trie_mut(&address_1)
            .unwrap()
            .update_leaves(&mut updates, |_, _| {})
            .unwrap();
        assert!(updates.is_empty());
        trie_account_1.storage_root = sparse.storage_root(&address_1).unwrap();
        apply_account_update(
            &mut sparse,
            address_1,
            LeafUpdate::Changed(alloy_rlp::encode(trie_account_1)),
        );

        sparse.wipe_storage(address_2).unwrap();
        trie_account_2.storage_root = sparse.storage_root(&address_2).unwrap();
        apply_account_update(
            &mut sparse,
            address_2,
            LeafUpdate::Changed(alloy_rlp::encode(trie_account_2)),
        );

        sparse.root().unwrap();

        let sparse_updates = sparse.take_trie_updates().unwrap();
        // TODO(alexey): assert against real state root calculation updates
        pretty_assertions::assert_eq!(
            sparse_updates,
            TrieUpdates {
                account_nodes: HashMap::default(),
                storage_tries: HashMap::from_iter([
                    (
                        b256!("0x1000000000000000000000000000000000000000000000000000000000000000"),
                        StorageTrieUpdates {
                            is_deleted: false,
                            storage_nodes: HashMap::default(),
                            removed_nodes: HashSet::from_iter([Nibbles::from_nibbles([0x1])])
                        }
                    ),
                    (
                        b256!("0x1100000000000000000000000000000000000000000000000000000000000000"),
                        StorageTrieUpdates {
                            is_deleted: true,
                            storage_nodes: HashMap::default(),
                            removed_nodes: HashSet::default()
                        }
                    )
                ]),
                removed_nodes: HashSet::default()
            }
        );
    }
}
