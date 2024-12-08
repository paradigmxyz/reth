use crate::{
    blinded::{BlindedProvider, BlindedProviderFactory, DefaultBlindedProviderFactory},
    RevealedSparseTrie, SparseTrie,
};
use alloy_primitives::{
    hex,
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::{Decodable, Encodable};
use reth_execution_errors::{SparseStateTrieError, SparseStateTrieResult, SparseTrieError};
use reth_primitives_traits::Account;
use reth_tracing::tracing::trace;
use reth_trie_common::{
    updates::{StorageTrieUpdates, TrieUpdates},
    MultiProof, Nibbles, TrieAccount, TrieNode, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use std::{fmt, iter::Peekable};

/// Sparse state trie representing lazy-loaded Ethereum state trie.
pub struct SparseStateTrie<F: BlindedProviderFactory = DefaultBlindedProviderFactory> {
    /// Blinded node provider factory.
    provider_factory: F,
    /// Sparse account trie.
    state: SparseTrie<F::AccountNodeProvider>,
    /// Sparse storage tries.
    storages: HashMap<B256, SparseTrie<F::StorageNodeProvider>>,
    /// Collection of revealed account and storage keys.
    revealed: HashMap<B256, HashSet<B256>>,
    /// Flag indicating whether trie updates should be retained.
    retain_updates: bool,
    /// Reusable buffer for RLP encoding of trie accounts.
    account_rlp_buf: Vec<u8>,
}

impl Default for SparseStateTrie {
    fn default() -> Self {
        Self {
            provider_factory: Default::default(),
            state: Default::default(),
            storages: Default::default(),
            revealed: Default::default(),
            retain_updates: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
        }
    }
}

impl<P: BlindedProviderFactory> fmt::Debug for SparseStateTrie<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SparseStateTrie")
            .field("state", &self.state)
            .field("storages", &self.storages)
            .field("revealed", &self.revealed)
            .field("retain_updates", &self.retain_updates)
            .field("account_rlp_buf", &hex::encode(&self.account_rlp_buf))
            .finish_non_exhaustive()
    }
}

impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: SparseTrie) -> Self {
        Self { state, ..Default::default() }
    }
}

impl<F: BlindedProviderFactory> SparseStateTrie<F> {
    /// Create new [`SparseStateTrie`] with blinded node provider factory.
    pub fn new(provider_factory: F) -> Self {
        Self {
            provider_factory,
            state: Default::default(),
            storages: Default::default(),
            revealed: Default::default(),
            retain_updates: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
        }
    }

    /// Set the retention of branch node updates and deletions.
    pub const fn with_updates(mut self, retain_updates: bool) -> Self {
        self.retain_updates = retain_updates;
        self
    }

    /// Returns `true` if account was already revealed.
    pub fn is_account_revealed(&self, account: &B256) -> bool {
        self.revealed.contains_key(account)
    }

    /// Returns `true` if storage slot for account was already revealed.
    pub fn is_storage_slot_revealed(&self, account: &B256, slot: &B256) -> bool {
        self.revealed.get(account).is_some_and(|slots| slots.contains(slot))
    }

    /// Returns mutable reference to storage sparse trie if it was revealed.
    pub fn storage_trie_mut(
        &mut self,
        account: &B256,
    ) -> Option<&mut RevealedSparseTrie<F::StorageNodeProvider>> {
        self.storages.get_mut(account).and_then(|e| e.as_revealed_mut())
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

        if self.is_account_revealed(&account) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_root_node(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.state.reveal_root_with_provider(
            self.provider_factory.account_node_provider(),
            root_node,
            None,
            self.retain_updates,
        )?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node, None)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default();

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

        if self.is_storage_slot_revealed(&account, &slot) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_root_node(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.storages.entry(account).or_default().reveal_root_with_provider(
            self.provider_factory.storage_node_provider(account),
            root_node,
            None,
            self.retain_updates,
        )?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node, None)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default().insert(slot);

        Ok(())
    }

    /// Reveal unknown trie paths from multiproof and the list of included accounts and slots.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_multiproof(
        &mut self,
        targets: HashMap<B256, HashSet<B256>>,
        multiproof: MultiProof,
    ) -> SparseStateTrieResult<()> {
        let account_subtree = multiproof.account_subtree.into_nodes_sorted();
        let mut account_nodes = account_subtree.into_iter().peekable();

        if let Some(root_node) = self.validate_root_node(&mut account_nodes)? {
            // Reveal root node if it wasn't already.
            let trie = self.state.reveal_root_with_provider(
                self.provider_factory.account_node_provider(),
                root_node,
                multiproof.branch_node_hash_masks.get(&Nibbles::default()).copied(),
                self.retain_updates,
            )?;

            // Reveal the remaining proof nodes.
            for (path, bytes) in account_nodes {
                let node = TrieNode::decode(&mut &bytes[..])?;
                let hash_mask = if let TrieNode::Branch(_) = node {
                    multiproof.branch_node_hash_masks.get(&path).copied()
                } else {
                    None
                };

                trace!(target: "trie::sparse", ?path, ?node, ?hash_mask, "Revealing account node");
                trie.reveal_node(path, node, hash_mask)?;
            }
        }

        for (account, storage_subtree) in multiproof.storages {
            let subtree = storage_subtree.subtree.into_nodes_sorted();
            let mut nodes = subtree.into_iter().peekable();

            if let Some(root_node) = self.validate_root_node(&mut nodes)? {
                // Reveal root node if it wasn't already.
                let trie = self.storages.entry(account).or_default().reveal_root_with_provider(
                    self.provider_factory.storage_node_provider(account),
                    root_node,
                    storage_subtree.branch_node_hash_masks.get(&Nibbles::default()).copied(),
                    self.retain_updates,
                )?;

                // Reveal the remaining proof nodes.
                for (path, bytes) in nodes {
                    let node = TrieNode::decode(&mut &bytes[..])?;
                    let hash_mask = if let TrieNode::Branch(_) = node {
                        storage_subtree.branch_node_hash_masks.get(&path).copied()
                    } else {
                        None
                    };

                    trace!(target: "trie::sparse", ?account, ?path, ?node, ?hash_mask, "Revealing storage node");
                    trie.reveal_node(path, node, hash_mask)?;
                }
            }
        }

        for (account, slots) in targets {
            self.revealed.entry(account).or_default().extend(slots);
        }

        Ok(())
    }

    /// Validates the root node of the proof and returns it if it exists and is valid.
    fn validate_root_node<I: Iterator<Item = (Nibbles, Bytes)>>(
        &self,
        proof: &mut Peekable<I>,
    ) -> SparseStateTrieResult<Option<TrieNode>> {
        let mut proof = proof.into_iter().peekable();

        // Validate root node.
        let Some((path, node)) = proof.next() else { return Ok(None) };
        if !path.is_empty() {
            return Err(SparseStateTrieError::InvalidRootNode { path, node })
        }

        // Decode root node and perform sanity check.
        let root_node = TrieNode::decode(&mut &node[..])?;
        if matches!(root_node, TrieNode::EmptyRoot) && proof.peek().is_some() {
            return Err(SparseStateTrieError::InvalidRootNode { path, node })
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

    /// Calculates the hashes of the nodes below the provided level.
    pub fn calculate_below_level(&mut self, level: usize) {
        self.state.calculate_below_level(level);
    }

    /// Returns storage sparse trie root if the trie has been revealed.
    pub fn storage_root(&mut self, account: B256) -> Option<B256> {
        self.storages.get_mut(&account).and_then(|trie| trie.root())
    }

    /// Returns sparse trie root if the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        self.state.root()
    }

    /// Returns [`TrieUpdates`] by taking the updates from the revealed sparse tries.
    ///
    /// Returns `None` if the accounts trie is not revealed.
    pub fn take_trie_updates(&mut self) -> Option<TrieUpdates> {
        self.state.as_revealed_mut().map(|state| {
            let updates = state.take_updates();
            TrieUpdates {
                account_nodes: updates.updated_nodes,
                removed_nodes: updates.removed_nodes,
                storage_tries: self
                    .storages
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
                    .collect(),
            }
        })
    }
}
impl<F> SparseStateTrie<F>
where
    F: BlindedProviderFactory,
    SparseTrieError: From<<F::AccountNodeProvider as BlindedProvider>::Error>
        + From<<F::StorageNodeProvider as BlindedProvider>::Error>,
{
    /// Update the account leaf node.
    pub fn update_account_leaf(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
    ) -> SparseStateTrieResult<()> {
        self.state.update_leaf(path, value)?;
        Ok(())
    }

    /// Update the leaf node of a storage trie at the provided address.
    pub fn update_storage_leaf(
        &mut self,
        address: B256,
        slot: Nibbles,
        value: Vec<u8>,
    ) -> SparseStateTrieResult<()> {
        if let Some(storage_trie) = self.storages.get_mut(&address) {
            Ok(storage_trie.update_leaf(slot, value)?)
        } else {
            Err(SparseStateTrieError::Sparse(SparseTrieError::Blind))
        }
    }

    /// Update or remove trie account based on new account info. This method will either recompute
    /// the storage root based on update storage trie or look it up from existing leaf value.
    ///
    /// If the new account info and storage trie are empty, the account leaf will be removed.
    pub fn update_account(&mut self, address: B256, account: Account) -> SparseStateTrieResult<()> {
        let nibbles = Nibbles::unpack(address);
        let storage_root = if let Some(storage_trie) = self.storages.get_mut(&address) {
            trace!(target: "trie::sparse", ?address, "Calculating storage root to update account");
            storage_trie.root().ok_or(SparseTrieError::Blind)?
        } else if self.revealed.contains_key(&address) {
            trace!(target: "trie::sparse", ?address, "Retrieving storage root from account leaf to update account");
            let state = self.state.as_revealed_mut().ok_or(SparseTrieError::Blind)?;
            // The account was revealed, either...
            if let Some(value) = state.get_leaf_value(&nibbles) {
                // ..it exists and we should take it's current storage root or...
                TrieAccount::decode(&mut &value[..])?.storage_root
            } else {
                // ...the account is newly created and the storage trie is empty.
                EMPTY_ROOT_HASH
            }
        } else {
            return Err(SparseTrieError::Blind.into())
        };

        if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
            trace!(target: "trie::sparse", ?address, "Removing account");
            self.remove_account_leaf(&nibbles)
        } else {
            trace!(target: "trie::sparse", ?address, "Updating account");
            self.account_rlp_buf.clear();
            TrieAccount::from((account, storage_root)).encode(&mut self.account_rlp_buf);
            self.update_account_leaf(nibbles, self.account_rlp_buf.clone())
        }
    }

    /// Remove the account leaf node.
    pub fn remove_account_leaf(&mut self, path: &Nibbles) -> SparseStateTrieResult<()> {
        self.state.remove_leaf(path)?;
        Ok(())
    }

    /// Update the leaf node of a storage trie at the provided address.
    pub fn remove_storage_leaf(
        &mut self,
        address: B256,
        slot: &Nibbles,
    ) -> SparseStateTrieResult<()> {
        if let Some(storage_trie) = self.storages.get_mut(&address) {
            Ok(storage_trie.remove_leaf(slot)?)
        } else {
            Err(SparseStateTrieError::Sparse(SparseTrieError::Blind))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, Bytes, U256};
    use alloy_rlp::EMPTY_STRING_CODE;
    use arbitrary::Arbitrary;
    use assert_matches::assert_matches;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::StorageTrieUpdates, HashBuilder, TrieAccount, EMPTY_ROOT_HASH};
    use reth_trie_common::{proof::ProofRetainer, StorageMultiProof, TrieMask};

    #[test]
    fn validate_root_node_first_node_not_root() {
        let sparse = SparseStateTrie::default();
        let proof = [(Nibbles::from_nibbles([0x1]), Bytes::from([EMPTY_STRING_CODE]))];
        assert_matches!(
            sparse.validate_root_node(&mut proof.into_iter().peekable(),),
            Err(SparseStateTrieError::InvalidRootNode { .. })
        );
    }

    #[test]
    fn validate_root_node_invalid_proof_with_empty_root() {
        let sparse = SparseStateTrie::default();
        let proof = [
            (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
            (Nibbles::from_nibbles([0x1]), Bytes::new()),
        ];
        assert_matches!(
            sparse.validate_root_node(&mut proof.into_iter().peekable(),),
            Err(SparseStateTrieError::InvalidRootNode { .. })
        );
    }

    #[test]
    fn reveal_account_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::default();
        assert_eq!(sparse.state, SparseTrie::Blind);

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

        let mut sparse = SparseStateTrie::default();
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
    fn take_trie_updates() {
        reth_tracing::init_test_tracing();

        // let mut rng = generators::rng();
        let mut rng = StdRng::seed_from_u64(1);

        let mut bytes = [0u8; 1024];
        rng.fill(bytes.as_mut_slice());

        let slot_1 = b256!("1000000000000000000000000000000000000000000000000000000000000000");
        let slot_path_1 = Nibbles::unpack(slot_1);
        let value_1 = U256::from(rng.gen::<u64>());
        let slot_2 = b256!("1100000000000000000000000000000000000000000000000000000000000000");
        let slot_path_2 = Nibbles::unpack(slot_2);
        let value_2 = U256::from(rng.gen::<u64>());
        let slot_3 = b256!("2000000000000000000000000000000000000000000000000000000000000000");
        let slot_path_3 = Nibbles::unpack(slot_3);
        let value_3 = U256::from(rng.gen::<u64>());

        let mut storage_hash_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([
                slot_path_1.clone(),
                slot_path_2.clone(),
            ]));
        storage_hash_builder.add_leaf(slot_path_1, &alloy_rlp::encode_fixed_size(&value_1));
        storage_hash_builder.add_leaf(slot_path_2, &alloy_rlp::encode_fixed_size(&value_2));

        let storage_root = storage_hash_builder.root();
        let storage_proof_nodes = storage_hash_builder.take_proof_nodes();
        let storage_branch_node_hash_masks = HashMap::from_iter([
            (Nibbles::default(), TrieMask::new(0b010)),
            (Nibbles::from_nibbles([0x1]), TrieMask::new(0b11)),
        ]);

        let address_1 = b256!("1000000000000000000000000000000000000000000000000000000000000000");
        let address_path_1 = Nibbles::unpack(address_1);
        let account_1 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
        let mut trie_account_1 = TrieAccount::from((account_1, storage_root));
        let address_2 = b256!("1100000000000000000000000000000000000000000000000000000000000000");
        let address_path_2 = Nibbles::unpack(address_2);
        let account_2 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
        let mut trie_account_2 = TrieAccount::from((account_2, EMPTY_ROOT_HASH));

        let mut hash_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([
                address_path_1.clone(),
                address_path_2.clone(),
            ]));
        hash_builder.add_leaf(address_path_1.clone(), &alloy_rlp::encode(trie_account_1));
        hash_builder.add_leaf(address_path_2.clone(), &alloy_rlp::encode(trie_account_2));

        let root = hash_builder.root();
        let proof_nodes = hash_builder.take_proof_nodes();

        let mut sparse = SparseStateTrie::default().with_updates(true);
        sparse
            .reveal_multiproof(
                HashMap::from_iter([
                    (address_1, HashSet::from_iter([slot_1, slot_2])),
                    (address_2, HashSet::from_iter([slot_1, slot_2])),
                ]),
                MultiProof {
                    account_subtree: proof_nodes,
                    branch_node_hash_masks: HashMap::from_iter([(
                        Nibbles::from_nibbles([0x1]),
                        TrieMask::new(0b00),
                    )]),
                    storages: HashMap::from_iter([
                        (
                            address_1,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes.clone(),
                                branch_node_hash_masks: storage_branch_node_hash_masks.clone(),
                            },
                        ),
                        (
                            address_2,
                            StorageMultiProof {
                                root,
                                subtree: storage_proof_nodes,
                                branch_node_hash_masks: storage_branch_node_hash_masks,
                            },
                        ),
                    ]),
                },
            )
            .unwrap();

        assert_eq!(sparse.root(), Some(root));

        let address_3 = b256!("2000000000000000000000000000000000000000000000000000000000000000");
        let address_path_3 = Nibbles::unpack(address_3);
        let account_3 = Account { nonce: account_1.nonce + 1, ..account_1 };
        let trie_account_3 = TrieAccount::from((account_3, EMPTY_ROOT_HASH));

        sparse.update_account_leaf(address_path_3, alloy_rlp::encode(trie_account_3)).unwrap();

        sparse.update_storage_leaf(address_1, slot_path_3, alloy_rlp::encode(value_3)).unwrap();
        trie_account_1.storage_root = sparse.storage_root(address_1).unwrap();
        sparse.update_account_leaf(address_path_1, alloy_rlp::encode(trie_account_1)).unwrap();

        sparse.wipe_storage(address_2).unwrap();
        trie_account_2.storage_root = sparse.storage_root(address_2).unwrap();
        sparse.update_account_leaf(address_path_2, alloy_rlp::encode(trie_account_2)).unwrap();

        sparse.root();

        let sparse_updates = sparse.take_trie_updates().unwrap();
        // TODO(alexey): assert against real state root calculation updates
        pretty_assertions::assert_eq!(
            sparse_updates,
            TrieUpdates {
                account_nodes: HashMap::default(),
                storage_tries: HashMap::from_iter([(
                    b256!("1100000000000000000000000000000000000000000000000000000000000000"),
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
