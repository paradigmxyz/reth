use crate::{
    blinded::{BlindedProvider, BlindedProviderFactory},
    traits::SparseTrieInterface,
    SerialSparseTrie, SparseTrie, TrieMasks,
};
use alloc::{collections::VecDeque, vec::Vec};
use alloy_primitives::{
    map::{B256Map, HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::{Decodable, Encodable};
use alloy_trie::proof::DecodedProofNodes;
use core::iter::Peekable;
use reth_execution_errors::{SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieErrorKind};
use reth_primitives_traits::Account;
use reth_trie_common::{
    proof::ProofNodes,
    updates::{StorageTrieUpdates, TrieUpdates},
    DecodedMultiProof, DecodedStorageMultiProof, MultiProof, Nibbles, RlpNode, StorageMultiProof,
    TrieAccount, TrieMask, TrieNode, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use tracing::trace;

#[derive(Debug)]
/// Sparse state trie representing lazy-loaded Ethereum state trie.
pub struct SparseStateTrie<
    A = SerialSparseTrie, // Account trie implementation
    S = SerialSparseTrie, // Storage trie implementation
> {
    /// Sparse account trie.
    state: SparseTrie<A>,
    /// Sparse storage tries.
    storages: B256Map<SparseTrie<S>>,
    /// Collection of revealed account trie paths.
    revealed_account_paths: HashSet<Nibbles>,
    /// Collection of revealed storage trie paths, per account.
    revealed_storage_paths: B256Map<HashSet<Nibbles>>,
    /// Flag indicating whether trie updates should be retained.
    retain_updates: bool,
    /// Reusable buffer for RLP encoding of trie accounts.
    account_rlp_buf: Vec<u8>,
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
            storages: Default::default(),
            revealed_account_paths: Default::default(),
            revealed_storage_paths: Default::default(),
            retain_updates: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }
}

#[cfg(test)]
impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: SparseTrie) -> Self {
        Self { state, ..Default::default() }
    }
}

impl<A, S> SparseStateTrie<A, S>
where
    A: SparseTrieInterface + Default,
    S: SparseTrieInterface + Default,
{
    /// Create new [`SparseStateTrie`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the retention of branch node updates and deletions.
    pub const fn with_updates(mut self, retain_updates: bool) -> Self {
        self.retain_updates = retain_updates;
        self
    }

    /// Set the accounts trie to the given `SparseTrie`.
    pub fn with_accounts_trie(mut self, trie: SparseTrie<A>) -> Self {
        self.state = trie;
        self
    }

    /// Takes the accounts trie.
    pub fn take_accounts_trie(&mut self) -> SparseTrie<A> {
        core::mem::take(&mut self.state)
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
        self.revealed_storage_paths
            .get(&account)
            .is_some_and(|slots| slots.contains(&Nibbles::unpack(slot)))
    }

    /// Returns reference to bytes representing leaf value for the target account.
    pub fn get_account_value(&self, account: &B256) -> Option<&Vec<u8>> {
        self.state.as_revealed_ref()?.get_leaf_value(&Nibbles::unpack(account))
    }

    /// Returns reference to bytes representing leaf value for the target account and storage slot.
    pub fn get_storage_slot_value(&self, account: &B256, slot: &B256) -> Option<&Vec<u8>> {
        self.storages.get(account)?.as_revealed_ref()?.get_leaf_value(&Nibbles::unpack(slot))
    }

    /// Returns reference to state trie if it was revealed.
    pub const fn state_trie_ref(&self) -> Option<&A> {
        self.state.as_revealed_ref()
    }

    /// Returns reference to storage trie if it was revealed.
    pub fn storage_trie_ref(&self, address: &B256) -> Option<&S> {
        self.storages.get(address).and_then(|e| e.as_revealed_ref())
    }

    /// Returns mutable reference to storage sparse trie if it was revealed.
    pub fn storage_trie_mut(&mut self, address: &B256) -> Option<&mut S> {
        self.storages.get_mut(address).and_then(|e| e.as_revealed_mut())
    }

    /// Takes the storage trie for the provided address.
    pub fn take_storage_trie(&mut self, address: &B256) -> Option<SparseTrie<S>> {
        self.storages.remove(address)
    }

    /// Inserts storage trie for the provided address.
    pub fn insert_storage_trie(&mut self, address: B256, storage_trie: SparseTrie<S>) {
        self.storages.insert(address, storage_trie);
    }

    /// Reveal unknown trie paths from provided leaf path and its proof for the account.
    ///
    /// Panics if trie updates retention is enabled.
    ///
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_account(
        &mut self,
        account: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        assert!(!self.retain_updates);

        if self.is_account_revealed(account) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_root_node(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.state.reveal_root(root_node, TrieMasks::none(), self.retain_updates)?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            if self.revealed_account_paths.contains(&path) {
                continue
            }
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node, TrieMasks::none())?;

            // Track the revealed path.
            self.revealed_account_paths.insert(path);
        }

        Ok(())
    }

    /// Reveal unknown trie paths from provided leaf path and its proof for the storage slot.
    ///
    /// Panics if trie updates retention is enabled.
    ///
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_storage_slot(
        &mut self,
        account: B256,
        slot: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        assert!(!self.retain_updates);

        if self.is_storage_slot_revealed(account, slot) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_root_node(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.storages.entry(account).or_default().reveal_root(
            root_node,
            TrieMasks::none(),
            self.retain_updates,
        )?;

        let revealed_nodes = self.revealed_storage_paths.entry(account).or_default();

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            // If the node is already revealed, skip it.
            if revealed_nodes.contains(&path) {
                continue
            }
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node, TrieMasks::none())?;

            // Track the revealed path.
            revealed_nodes.insert(path);
        }

        Ok(())
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
    pub fn reveal_decoded_multiproof(
        &mut self,
        multiproof: DecodedMultiProof,
    ) -> SparseStateTrieResult<()> {
        let DecodedMultiProof {
            account_subtree,
            storages,
            branch_node_hash_masks,
            branch_node_tree_masks,
        } = multiproof;

        // first reveal the account proof nodes
        self.reveal_decoded_account_multiproof(
            account_subtree,
            branch_node_hash_masks,
            branch_node_tree_masks,
        )?;

        // then reveal storage proof nodes for each storage trie
        for (account, storage_subtree) in storages {
            self.reveal_decoded_storage_multiproof(account, storage_subtree)?;
        }

        Ok(())
    }

    /// Reveals an account multiproof.
    pub fn reveal_account_multiproof(
        &mut self,
        account_subtree: ProofNodes,
        branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
        branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    ) -> SparseStateTrieResult<()> {
        // decode the multiproof first
        let decoded_multiproof = account_subtree.try_into()?;
        self.reveal_decoded_account_multiproof(
            decoded_multiproof,
            branch_node_hash_masks,
            branch_node_tree_masks,
        )
    }

    /// Reveals a decoded account multiproof.
    pub fn reveal_decoded_account_multiproof(
        &mut self,
        account_subtree: DecodedProofNodes,
        branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
        branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    ) -> SparseStateTrieResult<()> {
        let FilteredProofNodes {
            nodes,
            new_nodes: _,
            total_nodes: _total_nodes,
            skipped_nodes: _skipped_nodes,
        } = filter_revealed_nodes(account_subtree, &self.revealed_account_paths)?;
        #[cfg(feature = "metrics")]
        {
            self.metrics.increment_total_account_nodes(_total_nodes as u64);
            self.metrics.increment_skipped_account_nodes(_skipped_nodes as u64);
        }
        let mut account_nodes = nodes.into_iter().peekable();

        if let Some(root_node) = Self::validate_root_node_decoded(&mut account_nodes)? {
            // Reveal root node if it wasn't already.
            let trie = self.state.reveal_root(
                root_node,
                TrieMasks {
                    hash_mask: branch_node_hash_masks.get(&Nibbles::default()).copied(),
                    tree_mask: branch_node_tree_masks.get(&Nibbles::default()).copied(),
                },
                self.retain_updates,
            )?;

            // Reveal the remaining proof nodes.
            for (path, node) in account_nodes {
                let (hash_mask, tree_mask) = if let TrieNode::Branch(_) = node {
                    (
                        branch_node_hash_masks.get(&path).copied(),
                        branch_node_tree_masks.get(&path).copied(),
                    )
                } else {
                    (None, None)
                };

                trace!(target: "trie::sparse", ?path, ?node, ?hash_mask, ?tree_mask, "Revealing account node");
                trie.reveal_node(path, node, TrieMasks { hash_mask, tree_mask })?;

                // Track the revealed path.
                self.revealed_account_paths.insert(path);
            }
        }

        Ok(())
    }

    /// Reveals a storage multiproof for the given address.
    pub fn reveal_storage_multiproof(
        &mut self,
        account: B256,
        storage_subtree: StorageMultiProof,
    ) -> SparseStateTrieResult<()> {
        // decode the multiproof first
        let decoded_multiproof = storage_subtree.try_into()?;
        self.reveal_decoded_storage_multiproof(account, decoded_multiproof)
    }

    /// Reveals a decoded storage multiproof for the given address.
    pub fn reveal_decoded_storage_multiproof(
        &mut self,
        account: B256,
        storage_subtree: DecodedStorageMultiProof,
    ) -> SparseStateTrieResult<()> {
        let revealed_nodes = self.revealed_storage_paths.entry(account).or_default();

        let FilteredProofNodes {
            nodes,
            new_nodes,
            total_nodes: _total_nodes,
            skipped_nodes: _skipped_nodes,
        } = filter_revealed_nodes(storage_subtree.subtree, revealed_nodes)?;
        #[cfg(feature = "metrics")]
        {
            self.metrics.increment_total_storage_nodes(_total_nodes as u64);
            self.metrics.increment_skipped_storage_nodes(_skipped_nodes as u64);
        }
        let mut nodes = nodes.into_iter().peekable();

        if let Some(root_node) = Self::validate_root_node_decoded(&mut nodes)? {
            // Reveal root node if it wasn't already.
            let trie = self.storages.entry(account).or_default().reveal_root(
                root_node,
                TrieMasks {
                    hash_mask: storage_subtree
                        .branch_node_hash_masks
                        .get(&Nibbles::default())
                        .copied(),
                    tree_mask: storage_subtree
                        .branch_node_tree_masks
                        .get(&Nibbles::default())
                        .copied(),
                },
                self.retain_updates,
            )?;

            // Reserve the capacity for new nodes ahead of time.
            trie.reserve_nodes(new_nodes);

            // Reveal the remaining proof nodes.
            for (path, node) in nodes {
                let (hash_mask, tree_mask) = if let TrieNode::Branch(_) = node {
                    (
                        storage_subtree.branch_node_hash_masks.get(&path).copied(),
                        storage_subtree.branch_node_tree_masks.get(&path).copied(),
                    )
                } else {
                    (None, None)
                };

                trace!(target: "trie::sparse", ?account, ?path, ?node, ?hash_mask, ?tree_mask, "Revealing storage node");
                trie.reveal_node(path, node, TrieMasks { hash_mask, tree_mask })?;

                // Track the revealed path.
                revealed_nodes.insert(path);
            }
        }

        Ok(())
    }

    /// Reveal state witness with the given state root.
    /// The state witness is expected to be a map of `keccak(rlp(node)): rlp(node).`
    /// NOTE: This method does not extensively validate the witness.
    pub fn reveal_witness(
        &mut self,
        state_root: B256,
        witness: &B256Map<Bytes>,
    ) -> SparseStateTrieResult<()> {
        // Create a `(hash, path, maybe_account)` queue for traversing witness trie nodes
        // starting from the root node.
        let mut queue = VecDeque::from([(state_root, Nibbles::default(), None)]);

        while let Some((hash, path, maybe_account)) = queue.pop_front() {
            // Retrieve the trie node and decode it.
            let Some(trie_node_bytes) = witness.get(&hash) else { continue };
            let trie_node = TrieNode::decode(&mut &trie_node_bytes[..])?;

            // Push children nodes into the queue.
            match &trie_node {
                TrieNode::Branch(branch) => {
                    for (idx, maybe_child) in branch.as_ref().children() {
                        if let Some(child_hash) = maybe_child.and_then(RlpNode::as_hash) {
                            let mut child_path = path;
                            child_path.push_unchecked(idx);
                            queue.push_back((child_hash, child_path, maybe_account));
                        }
                    }
                }
                TrieNode::Extension(ext) => {
                    if let Some(child_hash) = ext.child.as_hash() {
                        let mut child_path = path;
                        child_path.extend(&ext.key);
                        queue.push_back((child_hash, child_path, maybe_account));
                    }
                }
                TrieNode::Leaf(leaf) => {
                    let mut full_path = path;
                    full_path.extend(&leaf.key);
                    if maybe_account.is_none() {
                        let hashed_address = B256::from_slice(&full_path.pack());
                        let account = TrieAccount::decode(&mut &leaf.value[..])?;
                        if account.storage_root != EMPTY_ROOT_HASH {
                            queue.push_back((
                                account.storage_root,
                                Nibbles::default(),
                                Some(hashed_address),
                            ));
                        }
                    }
                }
                TrieNode::EmptyRoot => {} // nothing to do here
            };

            // Reveal the node itself.
            if let Some(account) = maybe_account {
                // Check that the path was not already revealed.
                if self
                    .revealed_storage_paths
                    .get(&account)
                    .is_none_or(|paths| !paths.contains(&path))
                {
                    let storage_trie_entry = self.storages.entry(account).or_default();
                    if path.is_empty() {
                        // Handle special storage state root node case.
                        storage_trie_entry.reveal_root(
                            trie_node,
                            TrieMasks::none(),
                            self.retain_updates,
                        )?;
                    } else {
                        // Reveal non-root storage trie node.
                        storage_trie_entry
                            .as_revealed_mut()
                            .ok_or(SparseTrieErrorKind::Blind)?
                            .reveal_node(path, trie_node, TrieMasks::none())?;
                    }

                    // Track the revealed path.
                    self.revealed_storage_paths.entry(account).or_default().insert(path);
                }
            }
            // Check that the path was not already revealed.
            else if !self.revealed_account_paths.contains(&path) {
                if path.is_empty() {
                    // Handle special state root node case.
                    self.state.reveal_root(trie_node, TrieMasks::none(), self.retain_updates)?;
                } else {
                    // Reveal non-root state trie node.
                    self.state.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?.reveal_node(
                        path,
                        trie_node,
                        TrieMasks::none(),
                    )?;
                }

                // Track the revealed path.
                self.revealed_account_paths.insert(path);
            }
        }

        Ok(())
    }

    /// Validates the root node of the proof and returns it if it exists and is valid.
    fn validate_root_node<I: Iterator<Item = (Nibbles, Bytes)>>(
        &self,
        proof: &mut Peekable<I>,
    ) -> SparseStateTrieResult<Option<TrieNode>> {
        // Validate root node.
        let Some((path, node)) = proof.next() else { return Ok(None) };
        if !path.is_empty() {
            return Err(SparseStateTrieErrorKind::InvalidRootNode { path, node }.into())
        }

        // Decode root node and perform sanity check.
        let root_node = TrieNode::decode(&mut &node[..])?;
        if matches!(root_node, TrieNode::EmptyRoot) && proof.peek().is_some() {
            return Err(SparseStateTrieErrorKind::InvalidRootNode { path, node }.into())
        }

        Ok(Some(root_node))
    }

    /// Validates the decoded root node of the proof and returns it if it exists and is valid.
    fn validate_root_node_decoded<I: Iterator<Item = (Nibbles, TrieNode)>>(
        proof: &mut Peekable<I>,
    ) -> SparseStateTrieResult<Option<TrieNode>> {
        // Validate root node.
        let Some((path, root_node)) = proof.next() else { return Ok(None) };
        if !path.is_empty() {
            return Err(SparseStateTrieErrorKind::InvalidRootNode {
                path,
                node: alloy_rlp::encode(&root_node).into(),
            }
            .into())
        }

        // Perform sanity check.
        if matches!(root_node, TrieNode::EmptyRoot) && proof.peek().is_some() {
            return Err(SparseStateTrieErrorKind::InvalidRootNode {
                path,
                node: alloy_rlp::encode(&root_node).into(),
            }
            .into())
        }

        Ok(Some(root_node))
    }

    /// Wipe the storage trie at the provided address.
    pub fn wipe_storage(&mut self, address: B256) -> SparseStateTrieResult<()> {
        if let Some(trie) = self.storages.get_mut(&address) {
            trie.wipe()?;
        }
        Ok(())
    }

    /// Calculates the hashes of subtries.
    ///
    /// If the trie has not been revealed, this function does nothing.
    pub fn calculate_subtries(&mut self) {
        if let SparseTrie::Revealed(trie) = &mut self.state {
            trie.update_subtrie_hashes();
        }
    }

    /// Returns storage sparse trie root if the trie has been revealed.
    pub fn storage_root(&mut self, account: B256) -> Option<B256> {
        self.storages.get_mut(&account).and_then(|trie| trie.root())
    }

    /// Returns mutable reference to the revealed account sparse trie.
    ///
    /// If the trie is not revealed yet, its root will be revealed using the blinded node provider.
    fn revealed_trie_mut(
        &mut self,
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<&mut A> {
        match self.state {
            SparseTrie::Blind(_) => {
                let (root_node, hash_mask, tree_mask) = provider_factory
                    .account_node_provider()
                    .blinded_node(&Nibbles::default())?
                    .map(|node| {
                        TrieNode::decode(&mut &node.node[..])
                            .map(|decoded| (decoded, node.hash_mask, node.tree_mask))
                    })
                    .transpose()?
                    .unwrap_or((TrieNode::EmptyRoot, None, None));
                self.state
                    .reveal_root(root_node, TrieMasks { hash_mask, tree_mask }, self.retain_updates)
                    .map_err(Into::into)
            }
            SparseTrie::Revealed(ref mut trie) => Ok(trie),
        }
    }

    /// Returns sparse trie root.
    ///
    /// If the trie has not been revealed, this function reveals the root node and returns its hash.
    pub fn root(
        &mut self,
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<B256> {
        // record revealed node metrics
        #[cfg(feature = "metrics")]
        self.metrics.record();

        Ok(self.revealed_trie_mut(provider_factory)?.root())
    }

    /// Returns sparse trie root and trie updates if the trie has been revealed.
    pub fn root_with_updates(
        &mut self,
        provider_factory: impl BlindedProviderFactory,
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
        self.storages
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
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<()> {
        if !self.revealed_account_paths.contains(&path) {
            self.revealed_account_paths.insert(path);
        }

        let provider = provider_factory.account_node_provider();
        self.state.update_leaf(path, value, provider)?;
        Ok(())
    }

    /// Update the leaf node of a storage trie at the provided address.
    pub fn update_storage_leaf(
        &mut self,
        address: B256,
        slot: Nibbles,
        value: Vec<u8>,
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<()> {
        if !self.revealed_storage_paths.get(&address).is_some_and(|slots| slots.contains(&slot)) {
            self.revealed_storage_paths.entry(address).or_default().insert(slot);
        }

        let storage_trie = self.storages.get_mut(&address).ok_or(SparseTrieErrorKind::Blind)?;

        let provider = provider_factory.storage_node_provider(address);
        storage_trie.update_leaf(slot, value, provider)?;
        Ok(())
    }

    /// Update or remove trie account based on new account info. This method will either recompute
    /// the storage root based on update storage trie or look it up from existing leaf value.
    ///
    /// If the new account info and storage trie are empty, the account leaf will be removed.
    pub fn update_account(
        &mut self,
        address: B256,
        account: Account,
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<()> {
        let nibbles = Nibbles::unpack(address);

        let storage_root = if let Some(storage_trie) = self.storages.get_mut(&address) {
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
            trace!(target: "trie::sparse", ?address, "Removing account");
            self.remove_account_leaf(&nibbles, provider_factory)
        } else {
            trace!(target: "trie::sparse", ?address, "Updating account");
            self.account_rlp_buf.clear();
            account.into_trie_account(storage_root).encode(&mut self.account_rlp_buf);
            self.update_account_leaf(nibbles, self.account_rlp_buf.clone(), provider_factory)
        }
    }

    /// Update the storage root of a revealed account.
    ///
    /// If the account doesn't exist in the trie, the function is a no-op.
    ///
    /// If the new storage root is empty, and the account info was already empty, the account leaf
    /// will be removed.
    pub fn update_account_storage_root(
        &mut self,
        address: B256,
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<()> {
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
            return Ok(())
        };

        // Calculate the new storage root. If the storage trie doesn't exist, the storage root will
        // be empty.
        let storage_root = if let Some(storage_trie) = self.storages.get_mut(&address) {
            trace!(target: "trie::sparse", ?address, "Calculating storage root to update account");
            storage_trie.root().ok_or(SparseTrieErrorKind::Blind)?
        } else {
            EMPTY_ROOT_HASH
        };

        // Update the account with the new storage root.
        trie_account.storage_root = storage_root;

        let nibbles = Nibbles::unpack(address);
        if trie_account == TrieAccount::default() {
            // If the account is empty, remove it.
            trace!(target: "trie::sparse", ?address, "Removing account because the storage root is empty");
            self.remove_account_leaf(&nibbles, provider_factory)?;
        } else {
            // Otherwise, update the account leaf.
            trace!(target: "trie::sparse", ?address, "Updating account with the new storage root");
            self.account_rlp_buf.clear();
            trie_account.encode(&mut self.account_rlp_buf);
            self.update_account_leaf(nibbles, self.account_rlp_buf.clone(), provider_factory)?;
        }

        Ok(())
    }

    /// Remove the account leaf node.
    pub fn remove_account_leaf(
        &mut self,
        path: &Nibbles,
        provider_factory: impl BlindedProviderFactory,
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
        provider_factory: impl BlindedProviderFactory,
    ) -> SparseStateTrieResult<()> {
        let storage_trie = self.storages.get_mut(&address).ok_or(SparseTrieErrorKind::Blind)?;

        let provider = provider_factory.storage_node_provider(address);
        storage_trie.remove_leaf(slot, provider)?;
        Ok(())
    }
}

/// Result of [`filter_revealed_nodes`].
#[derive(Debug, PartialEq, Eq)]
struct FilteredProofNodes {
    /// Filtered, decoded and sorted proof nodes.
    nodes: Vec<(Nibbles, TrieNode)>,
    /// Number of nodes in the proof.
    total_nodes: usize,
    /// Number of nodes that were skipped because they were already revealed.
    skipped_nodes: usize,
    /// Number of new nodes that will be revealed. This includes all children of branch nodes, even
    /// if they are not in the proof.
    new_nodes: usize,
}

/// Filters the decoded nodes that are already revealed and returns additional information about the
/// number of total, skipped, and new nodes.
fn filter_revealed_nodes(
    proof_nodes: DecodedProofNodes,
    revealed_nodes: &HashSet<Nibbles>,
) -> alloy_rlp::Result<FilteredProofNodes> {
    let mut result = FilteredProofNodes {
        nodes: Vec::with_capacity(proof_nodes.len()),
        total_nodes: 0,
        skipped_nodes: 0,
        new_nodes: 0,
    };

    for (path, node) in proof_nodes.into_inner() {
        result.total_nodes += 1;
        // If the node is already revealed, skip it.
        if revealed_nodes.contains(&path) {
            result.skipped_nodes += 1;
            continue
        }

        result.new_nodes += 1;
        // If it's a branch node, increase the number of new nodes by the number of children
        // according to the state mask.
        if let TrieNode::Branch(branch) = &node {
            result.new_nodes += branch.state_mask.count_ones() as usize;
        }

        result.nodes.push((path, node));
    }

    result.nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blinded::DefaultBlindedProviderFactory;
    use alloy_primitives::{
        b256,
        map::{HashMap, HashSet},
        Bytes, U256,
    };
    use alloy_rlp::EMPTY_STRING_CODE;
    use arbitrary::Arbitrary;
    use assert_matches::assert_matches;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::StorageTrieUpdates, HashBuilder, MultiProof, EMPTY_ROOT_HASH};
    use reth_trie_common::{
        proof::{ProofNodes, ProofRetainer},
        BranchNode, LeafNode, StorageMultiProof, TrieMask,
    };

    #[test]
    fn validate_root_node_first_node_not_root() {
        let sparse = SparseStateTrie::<SerialSparseTrie>::default();
        let proof = [(Nibbles::from_nibbles([0x1]), Bytes::from([EMPTY_STRING_CODE]))];
        assert_matches!(
            sparse.validate_root_node(&mut proof.into_iter().peekable()).map_err(|e| e.into_kind()),
            Err(SparseStateTrieErrorKind::InvalidRootNode { .. })
        );
    }

    #[test]
    fn validate_root_node_invalid_proof_with_empty_root() {
        let sparse = SparseStateTrie::<SerialSparseTrie>::default();
        let proof = [
            (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
            (Nibbles::from_nibbles([0x1]), Bytes::new()),
        ];
        assert_matches!(
            sparse.validate_root_node(&mut proof.into_iter().peekable()).map_err(|e| e.into_kind()),
            Err(SparseStateTrieErrorKind::InvalidRootNode { .. })
        );
    }

    #[test]
    fn reveal_account_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::<SerialSparseTrie>::default();
        assert_eq!(sparse.state, SparseTrie::Blind(None));

        sparse.reveal_account(Default::default(), proofs.into_inner()).unwrap();
        assert_eq!(sparse.state, SparseTrie::revealed_empty());
    }

    #[test]
    fn reveal_storage_slot_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::<SerialSparseTrie>::default();
        assert!(sparse.storages.is_empty());

        sparse
            .reveal_storage_slot(Default::default(), Default::default(), proofs.into_inner())
            .unwrap();
        assert_eq!(
            sparse.storages,
            HashMap::from_iter([(Default::default(), SparseTrie::revealed_empty())])
        );
    }

    #[test]
    fn reveal_account_path_twice() {
        let provider_factory = DefaultBlindedProviderFactory;
        let mut sparse = SparseStateTrie::<SerialSparseTrie>::default();

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        let leaf_1 = alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(
            Nibbles::default(),
            leaf_value.clone(),
        )));
        let leaf_2 = alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(
            Nibbles::default(),
            leaf_value.clone(),
        )));

        let multiproof = MultiProof {
            account_subtree: ProofNodes::from_iter([
                (
                    Nibbles::default(),
                    alloy_rlp::encode(TrieNode::Branch(BranchNode {
                        stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                        state_mask: TrieMask::new(0b11),
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
        assert!(sparse
            .state_trie_ref()
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])),);
        assert_eq!(
            sparse.state_trie_ref().unwrap().get_leaf_value(&Nibbles::from_nibbles([0x0])),
            Some(&leaf_value)
        );

        // Remove the leaf node and check that the state trie does not contain the leaf node and
        // value
        sparse.remove_account_leaf(&Nibbles::from_nibbles([0x0]), &provider_factory).unwrap();
        assert!(!sparse
            .state_trie_ref()
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])),);
        assert!(sparse
            .state_trie_ref()
            .unwrap()
            .get_leaf_value(&Nibbles::from_nibbles([0x0]))
            .is_none());

        // Reveal multiproof again and check that the state trie still does not contain the leaf
        // node and value, because they were already revealed before
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        assert!(!sparse
            .state_trie_ref()
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])));
        assert!(sparse
            .state_trie_ref()
            .unwrap()
            .get_leaf_value(&Nibbles::from_nibbles([0x0]))
            .is_none());
    }

    #[test]
    fn reveal_storage_path_twice() {
        let provider_factory = DefaultBlindedProviderFactory;
        let mut sparse = SparseStateTrie::<SerialSparseTrie>::default();

        let leaf_value = alloy_rlp::encode(TrieAccount::default());
        let leaf_1 = alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(
            Nibbles::default(),
            leaf_value.clone(),
        )));
        let leaf_2 = alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(
            Nibbles::default(),
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
                            alloy_rlp::encode(TrieNode::Branch(BranchNode {
                                stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                                state_mask: TrieMask::new(0b11),
                            }))
                            .into(),
                        ),
                        (Nibbles::from_nibbles([0x0]), leaf_1.clone().into()),
                        (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
                    ]),
                    branch_node_hash_masks: Default::default(),
                    branch_node_tree_masks: Default::default(),
                },
            )]),
            ..Default::default()
        };

        // Reveal multiproof and check that the storage trie contains the leaf node and value
        sparse.reveal_decoded_multiproof(multiproof.clone().try_into().unwrap()).unwrap();
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])),);
        assert_eq!(
            sparse
                .storage_trie_ref(&B256::ZERO)
                .unwrap()
                .get_leaf_value(&Nibbles::from_nibbles([0x0])),
            Some(&leaf_value)
        );

        // Remove the leaf node and check that the storage trie does not contain the leaf node and
        // value
        sparse
            .remove_storage_leaf(B256::ZERO, &Nibbles::from_nibbles([0x0]), &provider_factory)
            .unwrap();
        assert!(!sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])),);
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&Nibbles::from_nibbles([0x0]))
            .is_none());

        // Reveal multiproof again and check that the storage trie still does not contain the leaf
        // node and value, because they were already revealed before
        sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
        assert!(!sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .nodes_ref()
            .contains_key(&Nibbles::from_nibbles([0x0])));
        assert!(sparse
            .storage_trie_ref(&B256::ZERO)
            .unwrap()
            .get_leaf_value(&Nibbles::from_nibbles([0x0]))
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
        let storage_branch_node_hash_masks = HashMap::from_iter([
            (Nibbles::default(), TrieMask::new(0b010)),
            (Nibbles::from_nibbles([0x1]), TrieMask::new(0b11)),
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

        let provider_factory = DefaultBlindedProviderFactory;
        let mut sparse = SparseStateTrie::<SerialSparseTrie>::default().with_updates(true);
        sparse
            .reveal_decoded_multiproof(
                MultiProof {
                    account_subtree: proof_nodes,
                    branch_node_hash_masks: HashMap::from_iter([(
                        Nibbles::from_nibbles([0x1]),
                        TrieMask::new(0b00),
                    )]),
                    branch_node_tree_masks: HashMap::default(),
                    storages: HashMap::from_iter([
                        (
                            address_1,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes.clone(),
                                branch_node_hash_masks: storage_branch_node_hash_masks.clone(),
                                branch_node_tree_masks: HashMap::default(),
                            },
                        ),
                        (
                            address_2,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes,
                                branch_node_hash_masks: storage_branch_node_hash_masks,
                                branch_node_tree_masks: HashMap::default(),
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
        trie_account_1.storage_root = sparse.storage_root(address_1).unwrap();
        sparse
            .update_account_leaf(
                address_path_1,
                alloy_rlp::encode(trie_account_1),
                &provider_factory,
            )
            .unwrap();

        sparse.wipe_storage(address_2).unwrap();
        trie_account_2.storage_root = sparse.storage_root(address_2).unwrap();
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

    #[test]
    fn test_filter_revealed_nodes() {
        let revealed_nodes = HashSet::from_iter([Nibbles::from_nibbles([0x0])]);
        let leaf = TrieNode::Leaf(LeafNode::new(Nibbles::default(), alloy_rlp::encode([])));
        let leaf_encoded = alloy_rlp::encode(&leaf);
        let branch = TrieNode::Branch(BranchNode::new(
            vec![RlpNode::from_rlp(&leaf_encoded), RlpNode::from_rlp(&leaf_encoded)],
            TrieMask::new(0b11),
        ));
        let proof_nodes = alloy_trie::proof::DecodedProofNodes::from_iter([
            (Nibbles::default(), branch.clone()),
            (Nibbles::from_nibbles([0x0]), leaf.clone()),
            (Nibbles::from_nibbles([0x1]), leaf.clone()),
        ]);

        let decoded = filter_revealed_nodes(proof_nodes, &revealed_nodes).unwrap();

        assert_eq!(
            decoded,
            FilteredProofNodes {
                nodes: vec![(Nibbles::default(), branch), (Nibbles::from_nibbles([0x1]), leaf)],
                // Branch, leaf, leaf
                total_nodes: 3,
                // Revealed leaf node with path 0x1
                skipped_nodes: 1,
                // Branch, two of its children, one leaf
                new_nodes: 4
            }
        );
    }
}
