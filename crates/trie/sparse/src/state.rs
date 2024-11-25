use std::iter::Peekable;

use crate::{SparseStateTrieError, SparseStateTrieResult, SparseTrie};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use reth_trie::{
    updates::{StorageTrieUpdates, TrieUpdates},
    Nibbles, TrieNode,
};

/// Sparse state trie representing lazy-loaded Ethereum state trie.
#[derive(Default, Debug)]
pub struct SparseStateTrie {
    retain_updates: bool,
    /// Sparse account trie.
    state: SparseTrie,
    /// Sparse storage tries.
    storages: HashMap<B256, SparseTrie>,
    /// Collection of revealed account and storage keys.
    revealed: HashMap<B256, HashSet<B256>>,
    /// Collection of addresses that had their storage tries wiped.
    wiped_storages: HashSet<B256>,
}

impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: SparseTrie) -> Self {
        Self { state, ..Default::default() }
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

    /// Reveal unknown trie paths from provided leaf path and its proof for the account.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_account(
        &mut self,
        account: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        if self.is_account_revealed(&account) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_proof(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.state.reveal_root(root_node, self.retain_updates)?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default();

        Ok(())
    }

    /// Reveal unknown trie paths from provided leaf path and its proof for the storage slot.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_storage_slot(
        &mut self,
        account: B256,
        slot: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        if self.is_storage_slot_revealed(&account, &slot) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_proof(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self
            .storages
            .entry(account)
            .or_default()
            .reveal_root(root_node, self.retain_updates)?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default().insert(slot);

        Ok(())
    }

    /// Validates the root node of the proof and returns it if it exists and is valid.
    fn validate_proof<I: Iterator<Item = (Nibbles, Bytes)>>(
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

    /// Update the account leaf node.
    pub fn update_account_leaf(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
    ) -> SparseStateTrieResult<()> {
        self.state.update_leaf(path, value)?;
        Ok(())
    }

    /// Remove the account leaf node.
    pub fn remove_account_leaf(&mut self, path: &Nibbles) -> SparseStateTrieResult<()> {
        self.state.remove_leaf(path)?;
        Ok(())
    }

    /// Returns sparse trie root if the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        self.state.root()
    }

    /// Calculates the hashes of the nodes below the provided level.
    pub fn calculate_below_level(&mut self, level: usize) {
        self.state.calculate_below_level(level);
    }

    /// Update the leaf node of a storage trie at the provided address.
    pub fn update_storage_leaf(
        &mut self,
        address: B256,
        slot: Nibbles,
        value: Vec<u8>,
    ) -> SparseStateTrieResult<()> {
        self.storages.entry(address).or_default().update_leaf(slot, value)?;
        Ok(())
    }

    /// Wipe the storage trie at the provided address.
    pub fn wipe_storage(&mut self, address: B256) -> SparseStateTrieResult<()> {
        let Some(trie) = self.storages.get_mut(&address) else { return Ok(()) };
        self.wiped_storages.insert(address);
        trie.wipe().map_err(Into::into)
    }

    /// Returns storage sparse trie root if the trie has been revealed.
    pub fn storage_root(&mut self, account: B256) -> Option<B256> {
        self.storages.get_mut(&account).and_then(|trie| trie.root())
    }

    /// Returns [`TrieUpdates`] by taking the updates from the revealed sparse tries.
    ///
    /// Returns `None` if the accounts trie is not revealed.
    pub fn take_trie_updates(&mut self) -> Option<TrieUpdates> {
        self.state.as_revealed_mut().map(|state| {
            let updates = state.take_updates();
            TrieUpdates {
                account_nodes: HashMap::from_iter(updates.updated_nodes),
                removed_nodes: HashSet::from_iter(updates.removed_nodes),
                storage_tries: self
                    .storages
                    .iter_mut()
                    .map(|(address, trie)| {
                        let trie = trie.as_revealed_mut().unwrap();
                        let updates = trie.take_updates();
                        let updates = StorageTrieUpdates {
                            is_deleted: self.wiped_storages.contains(address),
                            storage_nodes: HashMap::from_iter(updates.updated_nodes),
                            removed_nodes: HashSet::from_iter(updates.removed_nodes),
                        };
                        (*address, updates)
                    })
                    .filter(|(_, updates)| !updates.is_empty())
                    .collect(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, Bytes, U256};
    use alloy_rlp::EMPTY_STRING_CODE;
    use arbitrary::Arbitrary;
    use assert_matches::assert_matches;
    use itertools::Itertools;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_primitives_traits::Account;
    use reth_trie::{
        updates::StorageTrieUpdates, BranchNodeCompact, HashBuilder, TrieAccount, TrieMask,
        EMPTY_ROOT_HASH,
    };
    use reth_trie_common::proof::ProofRetainer;

    #[test]
    fn validate_proof_first_node_not_root() {
        let sparse = SparseStateTrie::default();
        let proof = [(Nibbles::from_nibbles([0x1]), Bytes::from([EMPTY_STRING_CODE]))];
        assert_matches!(
            sparse.validate_proof(&mut proof.into_iter().peekable()),
            Err(SparseStateTrieError::InvalidRootNode { .. })
        );
    }

    #[test]
    fn validate_proof_invalid_proof_with_empty_root() {
        let sparse = SparseStateTrie::default();
        let proof = [
            (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
            (Nibbles::from_nibbles([0x1]), Bytes::new()),
        ];
        assert_matches!(
            sparse.validate_proof(&mut proof.into_iter().peekable()),
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
        storage_hash_builder.add_leaf(slot_path_1.clone(), &alloy_rlp::encode_fixed_size(&value_1));
        storage_hash_builder.add_leaf(slot_path_2.clone(), &alloy_rlp::encode_fixed_size(&value_2));

        let storage_root = storage_hash_builder.root();
        let proof_nodes = storage_hash_builder.take_proof_nodes();
        let storage_proof_1 = proof_nodes
            .iter()
            .filter(|(path, _)| path.is_empty() || slot_path_1.common_prefix_length(path) > 0)
            .map(|(path, proof)| (path.clone(), proof.clone()))
            .sorted_by_key(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let storage_proof_2 = proof_nodes
            .iter()
            .filter(|(path, _)| path.is_empty() || slot_path_2.common_prefix_length(path) > 0)
            .map(|(path, proof)| (path.clone(), proof.clone()))
            .sorted_by_key(|(path, _)| path.clone())
            .collect::<Vec<_>>();

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
        let proof_1 = proof_nodes
            .iter()
            .filter(|(path, _)| path.is_empty() || address_path_1.common_prefix_length(path) > 0)
            .map(|(path, proof)| (path.clone(), proof.clone()))
            .sorted_by_key(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let proof_2 = proof_nodes
            .iter()
            .filter(|(path, _)| path.is_empty() || address_path_2.common_prefix_length(path) > 0)
            .map(|(path, proof)| (path.clone(), proof.clone()))
            .sorted_by_key(|(path, _)| path.clone())
            .collect::<Vec<_>>();

        let mut sparse = SparseStateTrie::default().with_updates(true);
        sparse.reveal_account(address_1, proof_1).unwrap();
        sparse.reveal_account(address_2, proof_2).unwrap();
        sparse.reveal_storage_slot(address_1, slot_1, storage_proof_1.clone()).unwrap();
        sparse.reveal_storage_slot(address_1, slot_2, storage_proof_2.clone()).unwrap();
        sparse.reveal_storage_slot(address_2, slot_1, storage_proof_1).unwrap();
        sparse.reveal_storage_slot(address_2, slot_2, storage_proof_2).unwrap();

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
                account_nodes: HashMap::from_iter([
                    (
                        Nibbles::default(),
                        BranchNodeCompact {
                            state_mask: TrieMask::new(0b110),
                            tree_mask: TrieMask::new(0b000),
                            hash_mask: TrieMask::new(0b010),
                            hashes: vec![b256!(
                                "4c4ffbda3569fcf2c24ea2000b4cec86ef8b92cbf9ff415db43184c0f75a212e"
                            )],
                            root_hash: Some(b256!(
                                "60944bd29458529c3065d19f63c6e3d5269596fd3b04ca2e7b318912dc89ca4c"
                            ))
                        },
                    ),
                ]),
                storage_tries: HashMap::from_iter([
                    (
                        b256!("1000000000000000000000000000000000000000000000000000000000000000"),
                        StorageTrieUpdates {
                            is_deleted: false,
                            storage_nodes: HashMap::from_iter([(
                                Nibbles::default(),
                                BranchNodeCompact {
                                    state_mask: TrieMask::new(0b110),
                                    tree_mask: TrieMask::new(0b000),
                                    hash_mask: TrieMask::new(0b010),
                                    hashes: vec![b256!("5bc8b4fdf51839c1e18b8d6a4bd3e2e52c9f641860f0e4d197b68c2679b0e436")],
                                    root_hash: Some(b256!("c44abf1a9e1a92736ac479b20328e8d7998aa8838b6ef52620324c9ce85e3201"))
                                }
                            )]),
                            removed_nodes: HashSet::default()
                        }
                    ),
                    (
                        b256!("1100000000000000000000000000000000000000000000000000000000000000"),
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
