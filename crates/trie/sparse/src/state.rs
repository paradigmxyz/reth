#[cfg(feature = "trie-debug")]
use crate::debug_recorder::TrieDebugRecorder;
use crate::{
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    traits::SparseTrie as SparseTrieTrait,
    ParallelSparseTrie, RevealableSparseTrie,
};
use alloc::vec::Vec;
use alloy_primitives::{
    map::{B256Map, B256Set, HashSet},
    B256,
};
use alloy_rlp::{Decodable, Encodable};
use reth_execution_errors::{SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieErrorKind};
use reth_primitives_traits::Account;
#[cfg(feature = "std")]
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_common::{
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeMasks, DecodedMultiProof, MultiProof, Nibbles, ProofTrieNodeV2, TrieAccount,
    TrieNodeV2, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
#[cfg(feature = "std")]
use tracing::debug;
use tracing::{instrument, trace};

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
    A = ParallelSparseTrie, // Account trie implementation
    S = ParallelSparseTrie, // Storage trie implementation
> {
    /// Sparse account trie.
    state: RevealableSparseTrie<A>,
    /// Collection of revealed account trie paths.
    revealed_account_paths: HashSet<Nibbles>,
    /// State related to storage tries.
    storage: StorageTries<S>,
    /// Flag indicating whether trie updates should be retained.
    retain_updates: bool,
    /// When true, skip filtering of V2 proof nodes that have already been revealed.
    /// This is useful when the sparse trie is being reused across blocks and already
    /// tracks revealed nodes internally.
    skip_proof_node_filtering: bool,
    /// Reusable buffer for RLP encoding of trie accounts.
    account_rlp_buf: Vec<u8>,
    /// Holds data that should be dropped after final state root is calculated.
    deferred_drops: DeferredDrops,
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
            revealed_account_paths: Default::default(),
            storage: Default::default(),
            retain_updates: false,
            skip_proof_node_filtering: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            deferred_drops: DeferredDrops::default(),
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

    /// Set whether to skip filtering of V2 proof nodes.
    ///
    /// When true, `reveal_*_v2_proof_nodes` methods will pass all nodes directly to the
    /// sparse trie without filtering already-revealed paths. This is useful when the
    /// sparse trie is being reused across blocks and handles node deduplication internally.
    pub const fn with_skip_proof_node_filtering(mut self, skip: bool) -> Self {
        self.skip_proof_node_filtering = skip;
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

    /// Returns `true` if account was already revealed.
    pub fn is_account_revealed(&self, account: B256) -> bool {
        self.revealed_account_paths.contains(&Nibbles::unpack(account))
    }

    /// Was the account witness for `address` complete?
    pub fn check_valid_account_witness(&self, address: B256) -> bool {
        let path = Nibbles::unpack(address);
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

    /// Returns `true` if storage slot for account was already revealed.
    pub fn is_storage_slot_revealed(&self, account: B256, slot: B256) -> bool {
        self.storage
            .revealed_paths
            .get(&account)
            .is_some_and(|slots| slots.contains(&Nibbles::unpack(slot)))
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
        // Reveal the account proof nodes.
        //
        // Skip revealing account nodes if this result only contains storage proofs.
        // `reveal_account_v2_proof_nodes` will return an error if empty `nodes` are passed into it
        // before the accounts trie root was revealed. This might happen in cases when first account
        // trie proof arrives later than first storage trie proof even though the account trie proof
        // was requested first.
        if !multiproof.account_proofs.is_empty() {
            self.reveal_account_v2_proof_nodes(multiproof.account_proofs)?;
        }

        #[cfg(not(feature = "std"))]
        // If nostd then serially reveal storage proof nodes for each storage trie
        {
            for (account, storage_proofs) in multiproof.storage_proofs {
                self.reveal_storage_v2_proof_nodes(account, storage_proofs)?;
                // Mark this storage trie as hot (accessed this tick)
                self.storage.modifications.mark_accessed(account);
            }

            Ok(())
        }

        #[cfg(feature = "std")]
        // If std then reveal storage proofs in parallel
        {
            use rayon::iter::ParallelIterator;
            use reth_primitives_traits::ParallelBridgeBuffered;

            let retain_updates = self.retain_updates;
            let skip_filtering = self.skip_proof_node_filtering;

            // Process all storage trie revealings in parallel, having first removed the
            // `reveal_nodes` tracking and `RevealableSparseTrie`s for each account from their
            // HashMaps. These will be returned after processing.
            let results: Vec<_> = multiproof
                .storage_proofs
                .into_iter()
                .map(|(account, storage_proofs)| {
                    let revealed_nodes = self.storage.take_or_create_revealed_paths(&account);
                    let trie = self.storage.take_or_create_trie(&account);
                    (account, storage_proofs, revealed_nodes, trie)
                })
                .par_bridge_buffered()
                .map(|(account, storage_proofs, mut revealed_nodes, mut trie)| {
                    let mut bufs = Vec::new();
                    let result = Self::reveal_storage_v2_proof_nodes_inner(
                        account,
                        storage_proofs,
                        &mut revealed_nodes,
                        &mut trie,
                        &mut bufs,
                        retain_updates,
                        skip_filtering,
                    );
                    (account, result, revealed_nodes, trie, bufs)
                })
                .collect();

            let mut any_err = Ok(());
            for (account, result, revealed_nodes, trie, bufs) in results {
                self.storage.revealed_paths.insert(account, revealed_nodes);
                self.storage.tries.insert(account, trie);
                // Mark this storage trie as hot (accessed this tick)
                self.storage.modifications.mark_accessed(account);
                if let Ok(_metric_values) = result {
                    #[cfg(feature = "metrics")]
                    {
                        self.metrics
                            .increment_total_storage_nodes(_metric_values.total_nodes as u64);
                        self.metrics
                            .increment_skipped_storage_nodes(_metric_values.skipped_nodes as u64);
                    }
                } else {
                    any_err = result.map(|_| ());
                }

                // Keep buffers for deferred dropping
                self.deferred_drops.proof_nodes_bufs.extend(bufs);
            }

            any_err
        }
    }

    /// Reveals account proof nodes from a V2 proof.
    ///
    /// V2 proofs already include the masks in the `ProofTrieNode` structure,
    /// so no separate masks map is needed.
    pub fn reveal_account_v2_proof_nodes(
        &mut self,
        mut nodes: Vec<ProofTrieNodeV2>,
    ) -> SparseStateTrieResult<()> {
        if self.skip_proof_node_filtering {
            let capacity = estimate_v2_proof_capacity(&nodes);

            #[cfg(feature = "metrics")]
            self.metrics.increment_total_account_nodes(nodes.len() as u64);

            let root_node = nodes.iter().find(|n| n.path.is_empty());
            let trie = if let Some(root_node) = root_node {
                trace!(target: "trie::sparse", ?root_node, "Revealing root account node from V2 proof");
                self.state.reveal_root(
                    root_node.node.clone(),
                    root_node.masks,
                    self.retain_updates,
                )?
            } else {
                self.state.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?
            };
            trie.reserve_nodes(capacity);
            trace!(target: "trie::sparse", total_nodes = ?nodes.len(), "Revealing account nodes from V2 proof (unfiltered)");
            trie.reveal_nodes(&mut nodes)?;

            self.deferred_drops.proof_nodes_bufs.push(nodes);
            return Ok(())
        }

        let FilteredV2ProofNodes { root_node, mut nodes, new_nodes, metric_values: _metric_values } =
            filter_revealed_v2_proof_nodes(nodes, &mut self.revealed_account_paths)?;

        #[cfg(feature = "metrics")]
        {
            self.metrics.increment_total_account_nodes(_metric_values.total_nodes as u64);
            self.metrics.increment_skipped_account_nodes(_metric_values.skipped_nodes as u64);
        }

        let trie = if let Some(root_node) = root_node {
            trace!(target: "trie::sparse", ?root_node, "Revealing root account node from V2 proof");
            self.state.reveal_root(root_node.node, root_node.masks, self.retain_updates)?
        } else {
            self.state.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?
        };

        trie.reserve_nodes(new_nodes);

        trace!(target: "trie::sparse", total_nodes = ?nodes.len(), "Revealing account nodes from V2 proof");
        trie.reveal_nodes(&mut nodes)?;

        // Keep buffer for deferred dropping
        self.deferred_drops.proof_nodes_bufs.push(nodes);

        Ok(())
    }

    /// Reveals storage proof nodes from a V2 proof for the given address.
    ///
    /// V2 proofs already include the masks in the `ProofTrieNode` structure,
    /// so no separate masks map is needed.
    pub fn reveal_storage_v2_proof_nodes(
        &mut self,
        account: B256,
        nodes: Vec<ProofTrieNodeV2>,
    ) -> SparseStateTrieResult<()> {
        let (trie, revealed_paths) = self.storage.get_trie_and_revealed_paths_mut(account);
        let _metric_values = Self::reveal_storage_v2_proof_nodes_inner(
            account,
            nodes,
            revealed_paths,
            trie,
            &mut self.deferred_drops.proof_nodes_bufs,
            self.retain_updates,
            self.skip_proof_node_filtering,
        )?;

        #[cfg(feature = "metrics")]
        {
            self.metrics.increment_total_storage_nodes(_metric_values.total_nodes as u64);
            self.metrics.increment_skipped_storage_nodes(_metric_values.skipped_nodes as u64);
        }

        Ok(())
    }

    /// Reveals storage V2 proof nodes for the given address. This is an internal static function
    /// designed to handle a variety of associated public functions.
    fn reveal_storage_v2_proof_nodes_inner(
        account: B256,
        mut nodes: Vec<ProofTrieNodeV2>,
        revealed_nodes: &mut HashSet<Nibbles>,
        trie: &mut RevealableSparseTrie<S>,
        bufs: &mut Vec<Vec<ProofTrieNodeV2>>,
        retain_updates: bool,
        skip_filtering: bool,
    ) -> SparseStateTrieResult<ProofNodesMetricValues> {
        if skip_filtering {
            let capacity = estimate_v2_proof_capacity(&nodes);
            let metric_values =
                ProofNodesMetricValues { total_nodes: nodes.len(), skipped_nodes: 0 };

            let root_node = nodes.iter().find(|n| n.path.is_empty());
            let trie = if let Some(root_node) = root_node {
                trace!(target: "trie::sparse", ?account, ?root_node, "Revealing root storage node from V2 proof");
                trie.reveal_root(root_node.node.clone(), root_node.masks, retain_updates)?
            } else {
                trie.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?
            };
            trie.reserve_nodes(capacity);
            trace!(target: "trie::sparse", ?account, total_nodes = ?nodes.len(), "Revealing storage nodes from V2 proof (unfiltered)");
            trie.reveal_nodes(&mut nodes)?;

            bufs.push(nodes);
            return Ok(metric_values)
        }

        let FilteredV2ProofNodes { root_node, mut nodes, new_nodes, metric_values } =
            filter_revealed_v2_proof_nodes(nodes, revealed_nodes)?;

        let trie = if let Some(root_node) = root_node {
            trace!(target: "trie::sparse", ?account, ?root_node, "Revealing root storage node from V2 proof");
            trie.reveal_root(root_node.node, root_node.masks, retain_updates)?
        } else {
            trie.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?
        };

        trie.reserve_nodes(new_nodes);

        trace!(target: "trie::sparse", ?account, total_nodes = ?nodes.len(), "Revealing storage nodes from V2 proof");
        trie.reveal_nodes(&mut nodes)?;

        // Keep buffer for deferred dropping
        bufs.push(nodes);

        Ok(metric_values)
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
    ///
    /// If the trie is not revealed yet, its root will be revealed using the trie node provider.
    fn revealed_trie_mut(
        &mut self,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<&mut A> {
        match self.state {
            RevealableSparseTrie::Blind(_) => {
                let (root_node, hash_mask, tree_mask) = provider_factory
                    .account_node_provider()
                    .trie_node(&Nibbles::default())?
                    .map(|node| {
                        TrieNodeV2::decode(&mut &node.node[..])
                            .map(|decoded| (decoded, node.hash_mask, node.tree_mask))
                    })
                    .transpose()?
                    .unwrap_or((TrieNodeV2::EmptyRoot, None, None));
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                self.state.reveal_root(root_node, masks, self.retain_updates).map_err(Into::into)
            }
            RevealableSparseTrie::Revealed(ref mut trie) => Ok(trie),
        }
    }

    /// Returns sparse trie root.
    ///
    /// If the trie has not been revealed, this function reveals the root node and returns its hash.
    pub fn root(
        &mut self,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<B256> {
        // record revealed node metrics
        #[cfg(feature = "metrics")]
        self.metrics.record();

        Ok(self.revealed_trie_mut(provider_factory)?.root())
    }

    /// Returns sparse trie root and trie updates if the trie has been revealed.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn root_with_updates(
        &mut self,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<(B256, TrieUpdates)> {
        // record revealed node metrics
        #[cfg(feature = "metrics")]
        self.metrics.record();

        let storage_tries = self.storage_trie_updates();
        let revealed = self.revealed_trie_mut(provider_factory)?;

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

    /// Update the account leaf node.
    pub fn update_account_leaf(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<()> {
        if !self.revealed_account_paths.contains(&path) {
            self.revealed_account_paths.insert(path);
        }

        let provider = provider_factory.account_node_provider();
        self.state.update_leaf(path, value, provider)?;
        Ok(())
    }

    /// Update the leaf node of a revealed storage trie at the provided address.
    pub fn update_storage_leaf(
        &mut self,
        address: B256,
        slot: Nibbles,
        value: Vec<u8>,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<()> {
        let provider = provider_factory.storage_node_provider(address);
        self.storage
            .tries
            .get_mut(&address)
            .ok_or(SparseTrieErrorKind::Blind)?
            .update_leaf(slot, value, provider)?;
        self.storage.get_revealed_paths_mut(address).insert(slot);
        Ok(())
    }

    /// Update or remove trie account based on new account info. This method will either recompute
    /// the storage root based on update storage trie or look it up from existing leaf value.
    ///
    /// Returns false if the new account info and storage trie are empty, indicating the account
    /// leaf should be removed.
    #[instrument(level = "trace", target = "trie::sparse", skip_all)]
    pub fn update_account(
        &mut self,
        address: B256,
        account: Account,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<bool> {
        let storage_root = if let Some(storage_trie) = self.storage.tries.get_mut(&address) {
            trace!(target: "trie::sparse", ?address, "Calculating storage root to update account");
            storage_trie.root().ok_or(SparseTrieErrorKind::Blind)?
        } else if self.is_account_revealed(address) {
            trace!(target: "trie::sparse", ?address, "Retrieving storage root from account leaf to update account");
            // The account was revealed, either...
            if let Some(value) = self.get_account_value(&address) {
                // ..it exists and we should take its current storage root or...
                TrieAccount::decode(&mut &value[..])?.storage_root
            } else {
                // ...the account is newly created and the storage trie is empty.
                EMPTY_ROOT_HASH
            }
        } else {
            return Err(SparseTrieErrorKind::Blind.into())
        };

        if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
            return Ok(false);
        }

        trace!(target: "trie::sparse", ?address, "Updating account");
        let nibbles = Nibbles::unpack(address);
        self.account_rlp_buf.clear();
        account.into_trie_account(storage_root).encode(&mut self.account_rlp_buf);
        self.update_account_leaf(nibbles, self.account_rlp_buf.clone(), provider_factory)?;

        Ok(true)
    }

    /// Update the storage root of a revealed account.
    ///
    /// If the account doesn't exist in the trie, the function is a no-op.
    ///
    /// Returns false if the new storage root is empty, and the account info was already empty,
    /// indicating the account leaf should be removed.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn update_account_storage_root(
        &mut self,
        address: B256,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<bool> {
        if !self.is_account_revealed(address) {
            return Err(SparseTrieErrorKind::Blind.into())
        }

        // Nothing to update if the account doesn't exist in the trie.
        let Some(mut trie_account) = self
            .get_account_value(&address)
            .map(|v| TrieAccount::decode(&mut &v[..]))
            .transpose()?
        else {
            trace!(target: "trie::sparse", ?address, "Account not found in trie, skipping storage root update");
            return Ok(true)
        };

        // Calculate the new storage root. If the storage trie doesn't exist, the storage root will
        // be empty.
        let storage_root = if let Some(storage_trie) = self.storage.tries.get_mut(&address) {
            trace!(target: "trie::sparse", ?address, "Calculating storage root to update account");
            storage_trie.root().ok_or(SparseTrieErrorKind::Blind)?
        } else {
            EMPTY_ROOT_HASH
        };

        // Update the account with the new storage root.
        trie_account.storage_root = storage_root;

        // If the account is empty, indicate that it should be removed.
        if trie_account == TrieAccount::default() {
            return Ok(false)
        }

        // Otherwise, update the account leaf.
        trace!(target: "trie::sparse", ?address, "Updating account with the new storage root");
        let nibbles = Nibbles::unpack(address);
        self.account_rlp_buf.clear();
        trie_account.encode(&mut self.account_rlp_buf);
        self.update_account_leaf(nibbles, self.account_rlp_buf.clone(), provider_factory)?;

        Ok(true)
    }

    /// Remove the account leaf node.
    #[instrument(level = "debug", target = "trie::sparse", skip_all)]
    pub fn remove_account_leaf(
        &mut self,
        path: &Nibbles,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<()> {
        let provider = provider_factory.account_node_provider();
        self.state.remove_leaf(path, provider)?;
        Ok(())
    }

    /// Update the leaf node of a storage trie at the provided address.
    pub fn remove_storage_leaf(
        &mut self,
        address: B256,
        slot: &Nibbles,
        provider_factory: impl TrieNodeProviderFactory,
    ) -> SparseStateTrieResult<()> {
        let storage_trie =
            self.storage.tries.get_mut(&address).ok_or(SparseTrieErrorKind::Blind)?;

        let provider = provider_factory.storage_node_provider(address);
        storage_trie.remove_leaf(slot, provider)?;
        Ok(())
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
        self.revealed_account_paths.clear();
        self.storage.clear();
        self.account_rlp_buf.clear();
    }

    /// Shrinks the capacity of the sparse trie to the given node and value sizes.
    ///
    /// This helps reduce memory usage when the trie has excess capacity.
    /// Distributes capacity equally among all tries (account + storage).
    pub fn shrink_to(&mut self, max_nodes: usize, max_values: usize) {
        // Count total number of storage tries (active + cleared)
        let storage_tries_count = self.storage.tries.len() + self.storage.cleared_tries.len();

        // Total tries = 1 account trie + all storage tries
        let total_tries = 1 + storage_tries_count;

        // Distribute capacity equally among all tries
        let nodes_per_trie = max_nodes / total_tries;
        let values_per_trie = max_values / total_tries;

        // Shrink the account trie
        self.state.shrink_nodes_to(nodes_per_trie);
        self.state.shrink_values_to(values_per_trie);

        // Give storage tries the remaining capacity after account trie allocation
        let storage_nodes = max_nodes.saturating_sub(nodes_per_trie);
        let storage_values = max_values.saturating_sub(values_per_trie);

        // Shrink all storage tries (they will redistribute internally)
        self.storage.shrink_to(storage_nodes, storage_values);
    }

    /// Prunes the account trie and selected storage tries to reduce memory usage.
    ///
    /// Storage tries not in the top `max_storage_tries` by revealed node count are cleared
    /// entirely.
    ///
    /// # Preconditions
    ///
    /// Node hashes must be computed via `root()` before calling this method. Otherwise, nodes
    /// cannot be converted to hash stubs and pruning will have no effect.
    ///
    /// # Effects
    ///
    /// - Clears `revealed_account_paths` and `revealed_paths` for all storage tries
    #[cfg(feature = "std")]
    #[instrument(
        level = "debug",
        name = "SparseStateTrie::prune",
        target = "trie::sparse",
        skip_all,
        fields(%max_depth, %max_storage_tries)
    )]
    pub fn prune(&mut self, max_depth: usize, max_storage_tries: usize) {
        // Prune state and storage tries in parallel
        rayon::join(
            || {
                if let Some(trie) = self.state.as_revealed_mut() {
                    trie.prune(max_depth);
                }
                self.revealed_account_paths.clear();
            },
            || {
                self.storage.prune(max_depth, max_storage_tries);
            },
        );
    }

    /// Commits the [`TrieUpdates`] to the sparse trie.
    pub fn commit_updates(&mut self, updates: &TrieUpdates) {
        if let Some(trie) = self.state.as_revealed_mut() {
            trie.commit_updates(&updates.account_nodes, &updates.removed_nodes);
        }
        for (address, updates) in &updates.storage_tries {
            if let Some(trie) =
                self.storage.tries.get_mut(address).and_then(|t| t.as_revealed_mut())
            {
                trie.commit_updates(&updates.storage_nodes, &updates.removed_nodes);
            }
        }
    }
}

/// The fields of [`SparseStateTrie`] related to storage tries. This is kept separate from the rest
/// of [`SparseStateTrie`] both to help enforce allocation re-use and to allow us to implement
/// methods like `get_trie_and_revealed_paths` which return multiple mutable borrows.
#[derive(Debug, Default)]
struct StorageTries<S = ParallelSparseTrie> {
    /// Sparse storage tries.
    tries: B256Map<RevealableSparseTrie<S>>,
    /// Cleared storage tries, kept for re-use.
    cleared_tries: Vec<RevealableSparseTrie<S>>,
    /// Collection of revealed storage trie paths, per account.
    revealed_paths: B256Map<HashSet<Nibbles>>,
    /// Cleared revealed storage trie path collections, kept for re-use.
    cleared_revealed_paths: Vec<HashSet<Nibbles>>,
    /// A default cleared trie instance, which will be cloned when creating new tries.
    default_trie: RevealableSparseTrie<S>,
    /// Tracks access patterns and modification state of storage tries for smart pruning decisions.
    modifications: StorageTrieModifications,
}

#[cfg(feature = "std")]
impl<S: SparseTrieTrait> StorageTries<S> {
    /// Prunes and evicts storage tries.
    ///
    /// Keeps the top `max_storage_tries` by a score combining size and heat.
    /// Evicts lower-scored tries entirely, prunes kept tries to `max_depth`.
    fn prune(&mut self, max_depth: usize, max_storage_tries: usize) {
        let fn_start = Instant::now();
        let mut stats =
            StorageTriesPruneStats { total_tries_before: self.tries.len(), ..Default::default() };

        // Update heat for accessed tries
        self.modifications.update_and_reset();

        // Collect (address, size, score) for all tries
        // Score = size * heat_multiplier
        // Hot tries (high heat) get boosted weight
        let mut trie_info: Vec<(B256, usize, usize)> = self
            .tries
            .iter()
            .map(|(address, trie)| {
                let size = match trie {
                    RevealableSparseTrie::Blind(_) => return (*address, 0, 0),
                    RevealableSparseTrie::Revealed(t) => t.size_hint(),
                };
                let heat = self.modifications.heat(address);
                // Heat multiplier: 1 (cold) to 3 (very hot, heat >= 4)
                let heat_multiplier = 1 + (heat.min(4) / 2) as usize;
                (*address, size, size * heat_multiplier)
            })
            .collect();

        // Use O(n) selection to find top max_storage_tries by score
        if trie_info.len() > max_storage_tries {
            trie_info
                .select_nth_unstable_by(max_storage_tries.saturating_sub(1), |a, b| b.2.cmp(&a.2));
            trie_info.truncate(max_storage_tries);
        }
        let tries_to_keep: B256Map<usize> =
            trie_info.iter().map(|(address, size, _)| (*address, *size)).collect();
        stats.tries_to_keep = tries_to_keep.len();

        // Collect keys to evict
        let tries_to_clear: Vec<B256> =
            self.tries.keys().filter(|addr| !tries_to_keep.contains_key(*addr)).copied().collect();
        stats.tries_to_evict = tries_to_clear.len();

        // Evict storage tries that exceeded limit, saving cleared allocations for reuse
        for address in &tries_to_clear {
            if let Some(mut trie) = self.tries.remove(address) {
                trie.clear();
                self.cleared_tries.push(trie);
            }
            if let Some(mut paths) = self.revealed_paths.remove(address) {
                paths.clear();
                self.cleared_revealed_paths.push(paths);
            }
            self.modifications.remove(address);
        }

        // Prune storage tries that are kept, but only if:
        // - They haven't been pruned since last access
        // - They're large enough to be worth pruning
        const MIN_SIZE_TO_PRUNE: usize = 1000;
        let prune_start = Instant::now();
        for (address, size) in &tries_to_keep {
            if *size < MIN_SIZE_TO_PRUNE {
                stats.skipped_small += 1;
                continue; // Small tries aren't worth the DFS cost
            }
            let Some(heat_state) = self.modifications.get_mut(address) else {
                continue; // No heat state = not tracked
            };
            // Only prune if backlog >= 2 (skip every other cycle)
            if heat_state.prune_backlog < 2 {
                stats.skipped_recently_pruned += 1;
                continue; // Recently pruned, skip this cycle
            }
            if let Some(trie) = self.tries.get_mut(address).and_then(|t| t.as_revealed_mut()) {
                trie.prune(max_depth);
                heat_state.prune_backlog = 0; // Reset backlog after prune
                stats.pruned_count += 1;
            }
        }
        stats.prune_elapsed = prune_start.elapsed();

        // Clear revealed_paths for kept tries
        for hash in tries_to_keep.keys() {
            if let Some(paths) = self.revealed_paths.get_mut(hash) {
                paths.clear();
            }
        }

        stats.total_tries_after = self.tries.len();
        stats.total_elapsed = fn_start.elapsed();

        debug!(
            target: "trie::sparse",
            before = stats.total_tries_before,
            after = stats.total_tries_after,
            kept = stats.tries_to_keep,
            evicted = stats.tries_to_evict,
            pruned = stats.pruned_count,
            skipped_small = stats.skipped_small,
            skipped_recent = stats.skipped_recently_pruned,
            ?stats.prune_elapsed,
            ?stats.total_elapsed,
            "StorageTries::prune completed"
        );
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
        self.cleared_revealed_paths.extend(self.revealed_paths.drain().map(|(_, mut set)| {
            set.clear();
            set
        }));
        self.modifications.clear();
    }

    /// Shrinks the capacity of all storage tries to the given total sizes.
    ///
    /// Distributes capacity equally among all tries (active + cleared).
    fn shrink_to(&mut self, max_nodes: usize, max_values: usize) {
        let total_tries = self.tries.len() + self.cleared_tries.len();
        if total_tries == 0 {
            return;
        }

        // Distribute capacity equally among all tries
        let nodes_per_trie = max_nodes / total_tries;
        let values_per_trie = max_values / total_tries;

        // Shrink active storage tries
        for trie in self.tries.values_mut() {
            trie.shrink_nodes_to(nodes_per_trie);
            trie.shrink_values_to(values_per_trie);
        }

        // Shrink cleared storage tries
        for trie in &mut self.cleared_tries {
            trie.shrink_nodes_to(nodes_per_trie);
            trie.shrink_values_to(values_per_trie);
        }
    }
}

impl<S: SparseTrieTrait + Clone> StorageTries<S> {
    /// Returns the set of already revealed trie node paths for an account's storage, creating the
    /// set if it didn't previously exist.
    fn get_revealed_paths_mut(&mut self, account: B256) -> &mut HashSet<Nibbles> {
        self.revealed_paths
            .entry(account)
            .or_insert_with(|| self.cleared_revealed_paths.pop().unwrap_or_default())
    }

    /// Returns the `RevealableSparseTrie` and the set of already revealed trie node paths for an
    /// account's storage, creating them if they didn't previously exist.
    fn get_trie_and_revealed_paths_mut(
        &mut self,
        account: B256,
    ) -> (&mut RevealableSparseTrie<S>, &mut HashSet<Nibbles>) {
        let trie = self.tries.entry(account).or_insert_with(|| {
            self.cleared_tries.pop().unwrap_or_else(|| self.default_trie.clone())
        });

        let revealed_paths = self
            .revealed_paths
            .entry(account)
            .or_insert_with(|| self.cleared_revealed_paths.pop().unwrap_or_default());

        (trie, revealed_paths)
    }

    // Returns mutable reference to storage sparse trie, creating a blind one if it doesn't exist.
    fn get_or_create_trie_mut(&mut self, address: B256) -> &mut RevealableSparseTrie<S> {
        self.tries.entry(address).or_insert_with(|| {
            self.cleared_tries.pop().unwrap_or_else(|| self.default_trie.clone())
        })
    }

    /// Takes the storage trie for the account from the internal `HashMap`, creating it if it
    /// doesn't already exist.
    #[cfg(feature = "std")]
    fn take_or_create_trie(&mut self, account: &B256) -> RevealableSparseTrie<S> {
        self.tries.remove(account).unwrap_or_else(|| {
            self.cleared_tries.pop().unwrap_or_else(|| self.default_trie.clone())
        })
    }

    /// Takes the revealed paths set from the account from the internal `HashMap`, creating one if
    /// it doesn't exist.
    #[cfg(feature = "std")]
    fn take_or_create_revealed_paths(&mut self, account: &B256) -> HashSet<Nibbles> {
        self.revealed_paths
            .remove(account)
            .unwrap_or_else(|| self.cleared_revealed_paths.pop().unwrap_or_default())
    }
}

/// Statistics from a storage tries prune operation.
#[derive(Debug, Default)]
#[allow(dead_code)]
struct StorageTriesPruneStats {
    total_tries_before: usize,
    total_tries_after: usize,
    tries_to_keep: usize,
    tries_to_evict: usize,
    pruned_count: usize,
    skipped_small: usize,
    skipped_recently_pruned: usize,
    prune_elapsed: core::time::Duration,
    total_elapsed: core::time::Duration,
}

/// Per-trie access tracking and prune state.
///
/// Tracks how frequently a storage trie is accessed and when it was last pruned,
/// enabling smart pruning decisions that preserve frequently-used tries.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
struct TrieModificationState {
    /// Access frequency level (0-255). Incremented each cycle the trie is accessed.
    /// Used for prioritizing which tries to keep during pruning.
    heat: u8,
    /// Prune backlog - cycles since last prune. Incremented each cycle,
    /// reset to 0 when pruned. Used to decide when pruning is needed.
    prune_backlog: u8,
}

/// Tracks access patterns and modification state of storage tries for smart pruning decisions.
///
/// Access-based tracking is more accurate than simple generation counting because it tracks
/// actual access patterns rather than administrative operations (take/insert).
///
/// - Access frequency is incremented when a storage proof is revealed (accessed)
/// - Access frequency decays each prune cycle for tries not accessed that cycle
/// - Tries with higher access frequency are prioritized for preservation during pruning
#[derive(Debug, Default)]
struct StorageTrieModifications {
    /// Access frequency and prune state per storage trie address.
    state: B256Map<TrieModificationState>,
    /// Tracks which tries were accessed in the current cycle (between prune calls).
    accessed_this_cycle: B256Set,
}

#[allow(dead_code)]
impl StorageTrieModifications {
    /// Marks a storage trie as accessed this cycle.
    /// Heat and `prune_backlog` are updated in [`Self::update_and_reset`].
    #[inline]
    fn mark_accessed(&mut self, address: B256) {
        self.accessed_this_cycle.insert(address);
    }

    /// Returns mutable reference to the heat state for a storage trie.
    #[inline]
    fn get_mut(&mut self, address: &B256) -> Option<&mut TrieModificationState> {
        self.state.get_mut(address)
    }

    /// Returns the heat level for a storage trie (0 if not tracked).
    #[inline]
    fn heat(&self, address: &B256) -> u8 {
        self.state.get(address).map_or(0, |s| s.heat)
    }

    /// Updates heat and prune backlog for accessed tries.
    /// Called at the start of each prune cycle.
    fn update_and_reset(&mut self) {
        for address in self.accessed_this_cycle.drain() {
            let entry = self.state.entry(address).or_default();
            entry.heat = entry.heat.saturating_add(1);
            entry.prune_backlog = entry.prune_backlog.saturating_add(1);
        }
    }

    /// Removes tracking for a specific address (when trie is evicted).
    fn remove(&mut self, address: &B256) {
        self.state.remove(address);
        self.accessed_this_cycle.remove(address);
    }

    /// Clears all heat tracking state.
    fn clear(&mut self) {
        self.state.clear();
        self.accessed_this_cycle.clear();
    }
}

#[derive(Debug, PartialEq, Eq, Default)]
struct ProofNodesMetricValues {
    /// Number of nodes in the proof.
    total_nodes: usize,
    /// Number of nodes that were skipped because they were already revealed.
    skipped_nodes: usize,
}

/// Result of [`filter_revealed_v2_proof_nodes`].
#[derive(Debug, PartialEq, Eq)]
struct FilteredV2ProofNodes {
    /// Root node which was pulled out of the original node set to be handled specially.
    root_node: Option<ProofTrieNodeV2>,
    /// Filtered proof nodes. Root node is removed.
    nodes: Vec<ProofTrieNodeV2>,
    /// Number of new nodes that will be revealed. This includes all children of branch nodes, even
    /// if they are not in the proof.
    new_nodes: usize,
    /// Values which are being returned so they can be incremented into metrics.
    metric_values: ProofNodesMetricValues,
}

/// Calculates capacity estimation for V2 proof nodes without filtering.
///
/// This counts nodes and their children (for branch and extension nodes) to provide
/// proper capacity hints for `reserve_nodes`. Used when `skip_proof_node_filtering` is
/// enabled and no filtering is performed.
fn estimate_v2_proof_capacity(nodes: &[ProofTrieNodeV2]) -> usize {
    let mut capacity = nodes.len();

    for node in nodes {
        if let TrieNodeV2::Branch(branch) = &node.node {
            capacity += branch.state_mask.count_ones() as usize;
        }
    }

    capacity
}

/// Filters V2 proof nodes that are already revealed, separates the root node if present, and
/// returns additional information about the number of total, skipped, and new nodes.
///
/// V2 proof nodes already have masks included in the `ProofTrieNode` structure, so no separate
/// masks map is needed.
fn filter_revealed_v2_proof_nodes(
    proof_nodes: Vec<ProofTrieNodeV2>,
    revealed_nodes: &mut HashSet<Nibbles>,
) -> SparseStateTrieResult<FilteredV2ProofNodes> {
    let mut result = FilteredV2ProofNodes {
        root_node: None,
        nodes: Vec::with_capacity(proof_nodes.len()),
        new_nodes: 0,
        metric_values: Default::default(),
    };

    // Count non-EmptyRoot nodes for sanity check. When multiple proofs are extended together,
    // duplicate EmptyRoot nodes may appear (e.g., storage proofs split across chunks for an
    // account with empty storage). We only error if there's an EmptyRoot alongside real nodes.
    let non_empty_root_count =
        proof_nodes.iter().filter(|n| !matches!(n.node, TrieNodeV2::EmptyRoot)).count();

    for node in proof_nodes {
        result.metric_values.total_nodes += 1;

        let is_root = node.path.is_empty();

        // If the node is already revealed, skip it. We don't ever skip the root node, nor do we add
        // it to `revealed_nodes`.
        if !is_root && !revealed_nodes.insert(node.path) {
            result.metric_values.skipped_nodes += 1;
            continue
        }

        result.new_nodes += 1;

        // Count children for capacity estimation
        if let TrieNodeV2::Branch(branch) = &node.node {
            result.new_nodes += branch.state_mask.count_ones() as usize;
        }

        if is_root {
            // Perform sanity check: EmptyRoot is only valid if there are no other real nodes.
            if matches!(node.node, TrieNodeV2::EmptyRoot) && non_empty_root_count > 0 {
                return Err(SparseStateTrieErrorKind::InvalidRootNode {
                    path: node.path,
                    node: {
                        let mut buf = Vec::new();
                        node.node.encode(&mut buf);
                        buf.into()
                    },
                }
                .into())
            }

            result.root_node = Some(node);
            continue
        }

        result.nodes.push(node);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{provider::DefaultTrieNodeProviderFactory, LeafLookup, ParallelSparseTrie};
    use alloy_primitives::{
        b256,
        map::{HashMap, HashSet},
        U256,
    };
    use arbitrary::Arbitrary;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::StorageTrieUpdates, HashBuilder, MultiProof, EMPTY_ROOT_HASH};
    use reth_trie_common::{
        proof::{ProofNodes, ProofRetainer},
        BranchNodeMasks, BranchNodeMasksMap, BranchNodeV2, LeafNode, RlpNode, StorageMultiProof,
        TrieMask,
    };

    /// Create a leaf key (suffix) with given nibbles padded with zeros to reach `total_len`.
    fn leaf_key(suffix: impl AsRef<[u8]>, total_len: usize) -> Nibbles {
        let suffix = suffix.as_ref();
        let mut nibbles = Nibbles::from_nibbles(suffix);
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![0; total_len - suffix.len()]));
        nibbles
    }

    #[test]
    fn reveal_account_path_twice() {
        let provider_factory = DefaultTrieNodeProviderFactory;
        let mut sparse = SparseStateTrie::<ParallelSparseTrie>::default();

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
        sparse.reveal_decoded_multiproof(multiproof.clone().try_into().unwrap()).unwrap();
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
        sparse.remove_account_leaf(&full_path_0, &provider_factory).unwrap();
        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::NonExistent)
        ));
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());

        // Reveal multiproof again and check that the state trie still does not contain the leaf
        // node and value, because they were already revealed before
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        assert!(matches!(
            sparse.state_trie_ref().unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::NonExistent)
        ));
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());
    }

    #[test]
    fn reveal_storage_path_twice() {
        let provider_factory = DefaultTrieNodeProviderFactory;
        let mut sparse = SparseStateTrie::<ParallelSparseTrie>::default();

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
        sparse.reveal_decoded_multiproof(multiproof.clone().try_into().unwrap()).unwrap();
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
        sparse.remove_storage_leaf(B256::ZERO, &full_path_0, &provider_factory).unwrap();
        assert!(matches!(
            sparse.storage_trie_ref(&B256::ZERO).unwrap().find_leaf(&full_path_0, None),
            Ok(LeafLookup::NonExistent)
        ));
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&full_path_0)
            .is_none());

        // Reveal multiproof again and check that the storage trie still does not contain the leaf
        // node and value, because they were already revealed before
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
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
    fn reveal_v2_proof_nodes() {
        let provider_factory = DefaultTrieNodeProviderFactory;
        let mut sparse = SparseStateTrie::<ParallelSparseTrie>::default();

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
        sparse.reveal_account_v2_proof_nodes(v2_proof_nodes.clone()).unwrap();

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
        sparse.remove_account_leaf(&full_path_0, &provider_factory).unwrap();
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());

        // Reveal again - should skip already revealed paths
        sparse.reveal_account_v2_proof_nodes(v2_proof_nodes).unwrap();
        assert!(sparse.state_trie_ref().unwrap().get_leaf_value(&full_path_0).is_none());
    }

    #[test]
    fn reveal_storage_v2_proof_nodes() {
        let provider_factory = DefaultTrieNodeProviderFactory;
        let mut sparse = SparseStateTrie::<ParallelSparseTrie>::default();

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
        sparse.reveal_storage_v2_proof_nodes(B256::ZERO, v2_proof_nodes.clone()).unwrap();

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
        sparse.remove_storage_leaf(B256::ZERO, &full_path_0, &provider_factory).unwrap();
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&full_path_0)
            .is_none());

        // Reveal again - should skip already revealed paths
        sparse.reveal_storage_v2_proof_nodes(B256::ZERO, v2_proof_nodes).unwrap();
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&full_path_0)
            .is_none());
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
        let slot_path_3 = Nibbles::unpack(slot_3);
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

        let provider_factory = DefaultTrieNodeProviderFactory;
        let mut sparse = SparseStateTrie::<ParallelSparseTrie>::default().with_updates(true);
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

        assert_eq!(sparse.root(&provider_factory).unwrap(), root);

        let address_3 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
        let address_path_3 = Nibbles::unpack(address_3);
        let account_3 = Account { nonce: account_1.nonce + 1, ..account_1 };
        let trie_account_3 = account_3.into_trie_account(EMPTY_ROOT_HASH);

        sparse
            .update_account_leaf(
                address_path_3,
                alloy_rlp::encode(trie_account_3),
                &provider_factory,
            )
            .unwrap();

        sparse
            .update_storage_leaf(
                address_1,
                slot_path_3,
                alloy_rlp::encode(value_3),
                &provider_factory,
            )
            .unwrap();
        trie_account_1.storage_root = sparse.storage_root(&address_1).unwrap();
        sparse
            .update_account_leaf(
                address_path_1,
                alloy_rlp::encode(trie_account_1),
                &provider_factory,
            )
            .unwrap();

        sparse.wipe_storage(address_2).unwrap();
        trie_account_2.storage_root = sparse.storage_root(&address_2).unwrap();
        sparse
            .update_account_leaf(
                address_path_2,
                alloy_rlp::encode(trie_account_2),
                &provider_factory,
            )
            .unwrap();

        sparse.root(&provider_factory).unwrap();

        let sparse_updates = sparse.take_trie_updates().unwrap();
        // TODO(alexey): assert against real state root calculation updates
        pretty_assertions::assert_eq!(
            sparse_updates,
            TrieUpdates {
                account_nodes: HashMap::default(),
                storage_tries: HashMap::from_iter([(
                    b256!("0x1100000000000000000000000000000000000000000000000000000000000000"),
                    StorageTrieUpdates {
                        is_deleted: true,
                        storage_nodes: HashMap::default(),
                        removed_nodes: HashSet::default()
                    }
                )]),
                removed_nodes: HashSet::default()
            }
        );
    }
}
