//! Merkle trie proofs.

use crate::{Nibbles, TrieAccount};
use alloc::{borrow::Cow, vec::Vec};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map, B256Set, HashMap},
    Address, Bytes, B256, U256,
};
use alloy_rlp::{encode_fixed_size, Decodable, EMPTY_STRING_CODE};
use alloy_trie::{
    nodes::TrieNode,
    proof::{verify_proof, DecodedProofNodes, ProofNodes, ProofVerificationError},
    TrieMask, EMPTY_ROOT_HASH,
};
use derive_more::{Deref, DerefMut, IntoIterator};
use itertools::Itertools;
use reth_primitives_traits::Account;

/// Proof targets map.
#[derive(Deref, DerefMut, IntoIterator, Clone, PartialEq, Eq, Default, Debug)]
pub struct MultiProofTargets(B256Map<B256Set>);

impl FromIterator<(B256, B256Set)> for MultiProofTargets {
    fn from_iter<T: IntoIterator<Item = (B256, B256Set)>>(iter: T) -> Self {
        Self(B256Map::from_iter(iter))
    }
}

impl MultiProofTargets {
    /// Creates an empty `MultiProofTargets` with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(B256Map::with_capacity_and_hasher(capacity, Default::default()))
    }

    /// Create `MultiProofTargets` with a single account as a target.
    pub fn account(hashed_address: B256) -> Self {
        Self::accounts([hashed_address])
    }

    /// Create `MultiProofTargets` with a single account and slots as targets.
    pub fn account_with_slots<I: IntoIterator<Item = B256>>(
        hashed_address: B256,
        slots_iter: I,
    ) -> Self {
        Self(B256Map::from_iter([(hashed_address, slots_iter.into_iter().collect())]))
    }

    /// Create `MultiProofTargets` only from accounts.
    pub fn accounts<I: IntoIterator<Item = B256>>(iter: I) -> Self {
        Self(iter.into_iter().map(|hashed_address| (hashed_address, Default::default())).collect())
    }

    /// Retains the targets representing the difference,
    /// i.e., the values that are in `self` but not in `other`.
    pub fn retain_difference(&mut self, other: &Self) {
        self.0.retain(|hashed_address, hashed_slots| {
            if let Some(other_hashed_slots) = other.get(hashed_address) {
                hashed_slots.retain(|hashed_slot| !other_hashed_slots.contains(hashed_slot));
                !hashed_slots.is_empty()
            } else {
                true
            }
        });
    }

    /// Extend multi proof targets with contents of other.
    pub fn extend(&mut self, other: Self) {
        self.extend_inner(Cow::Owned(other));
    }

    /// Extend multi proof targets with contents of other.
    ///
    /// Slightly less efficient than [`Self::extend`], but preferred to `extend(other.clone())`.
    pub fn extend_ref(&mut self, other: &Self) {
        self.extend_inner(Cow::Borrowed(other));
    }

    fn extend_inner(&mut self, other: Cow<'_, Self>) {
        for (hashed_address, hashed_slots) in other.iter() {
            self.entry(*hashed_address).or_default().extend(hashed_slots);
        }
    }

    /// Returns an iterator that yields chunks of the specified size.
    ///
    /// See [`ChunkedMultiProofTargets`] for more information.
    pub fn chunks(self, size: usize) -> ChunkedMultiProofTargets {
        ChunkedMultiProofTargets::new(self, size)
    }
}

/// An iterator that yields chunks of the proof targets of at most `size` account and storage
/// targets.
///
/// For example, for the following proof targets:
/// ```text
/// - 0x1: [0x10, 0x20, 0x30]
/// - 0x2: [0x40]
/// - 0x3: []
/// ```
///
/// and `size = 2`, the iterator will yield the following chunks:
/// ```text
/// - { 0x1: [0x10, 0x20] }
/// - { 0x1: [0x30], 0x2: [0x40] }
/// - { 0x3: [] }
/// ```
///
/// It follows two rules:
/// - If account has associated storage slots, each storage slot is counted towards the chunk size.
/// - If account has no associated storage slots, the account is counted towards the chunk size.
#[derive(Debug)]
pub struct ChunkedMultiProofTargets {
    flattened_targets: alloc::vec::IntoIter<(B256, Option<B256>)>,
    size: usize,
}

impl ChunkedMultiProofTargets {
    fn new(targets: MultiProofTargets, size: usize) -> Self {
        let flattened_targets = targets
            .into_iter()
            .flat_map(|(address, slots)| {
                if slots.is_empty() {
                    // If the account has no storage slots, we still need to yield the account
                    // address with empty storage slots. `None` here means that
                    // there's no storage slot to fetch.
                    itertools::Either::Left(core::iter::once((address, None)))
                } else {
                    itertools::Either::Right(
                        slots.into_iter().map(move |slot| (address, Some(slot))),
                    )
                }
            })
            .sorted();
        Self { flattened_targets, size }
    }
}

impl Iterator for ChunkedMultiProofTargets {
    type Item = MultiProofTargets;

    fn next(&mut self) -> Option<Self::Item> {
        let chunk = self.flattened_targets.by_ref().take(self.size).fold(
            MultiProofTargets::default(),
            |mut acc, (address, slot)| {
                let entry = acc.entry(address).or_default();
                if let Some(slot) = slot {
                    entry.insert(slot);
                }
                acc
            },
        );

        if chunk.is_empty() {
            None
        } else {
            Some(chunk)
        }
    }
}

/// The state multiproof of target accounts and multiproofs of their storage tries.
/// Multiproof is effectively a state subtrie that only contains the nodes
/// in the paths of target accounts.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct MultiProof {
    /// State trie multiproof for requested accounts.
    pub account_subtree: ProofNodes,
    /// The hash masks of the branch nodes in the account proof.
    pub branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// The tree masks of the branch nodes in the account proof.
    pub branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    /// Storage trie multiproofs.
    pub storages: B256Map<StorageMultiProof>,
}

impl MultiProof {
    /// Returns true if the multiproof is empty.
    pub fn is_empty(&self) -> bool {
        self.account_subtree.is_empty() &&
            self.branch_node_hash_masks.is_empty() &&
            self.branch_node_tree_masks.is_empty() &&
            self.storages.is_empty()
    }

    /// Return the account proof nodes for the given account path.
    pub fn account_proof_nodes(&self, path: &Nibbles) -> Vec<(Nibbles, Bytes)> {
        self.account_subtree.matching_nodes_sorted(path)
    }

    /// Return the storage proof nodes for the given storage slots of the account path.
    pub fn storage_proof_nodes(
        &self,
        hashed_address: B256,
        slots: impl IntoIterator<Item = B256>,
    ) -> Vec<(B256, Vec<(Nibbles, Bytes)>)> {
        self.storages
            .get(&hashed_address)
            .map(|storage_mp| {
                slots
                    .into_iter()
                    .map(|slot| {
                        let nibbles = Nibbles::unpack(slot);
                        (slot, storage_mp.subtree.matching_nodes_sorted(&nibbles))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Construct the account proof from the multiproof.
    pub fn account_proof(
        &self,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, alloy_rlp::Error> {
        let hashed_address = keccak256(address);
        let nibbles = Nibbles::unpack(hashed_address);

        // Retrieve the account proof.
        let proof = self
            .account_proof_nodes(&nibbles)
            .into_iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();

        // Inspect the last node in the proof. If it's a leaf node with matching suffix,
        // then the node contains the encoded trie account.
        let info = 'info: {
            if let Some(last) = proof.last() {
                if let TrieNode::Leaf(leaf) = TrieNode::decode(&mut &last[..])? {
                    if nibbles.ends_with(&leaf.key) {
                        let account = TrieAccount::decode(&mut &leaf.value[..])?;
                        break 'info Some(Account {
                            balance: account.balance,
                            nonce: account.nonce,
                            bytecode_hash: (account.code_hash != KECCAK_EMPTY)
                                .then_some(account.code_hash),
                        })
                    }
                }
            }
            None
        };

        // Retrieve proofs for requested storage slots.
        let storage_multiproof = self.storages.get(&hashed_address);
        let storage_root = storage_multiproof.map(|m| m.root).unwrap_or(EMPTY_ROOT_HASH);
        let mut storage_proofs = Vec::with_capacity(slots.len());
        for slot in slots {
            let proof = if let Some(multiproof) = &storage_multiproof {
                multiproof.storage_proof(*slot)?
            } else {
                StorageProof::new(*slot)
            };
            storage_proofs.push(proof);
        }
        Ok(AccountProof { address, info, proof, storage_root, storage_proofs })
    }

    /// Extends this multiproof with another one, merging both account and storage
    /// proofs.
    pub fn extend(&mut self, other: Self) {
        self.account_subtree.extend_from(other.account_subtree);

        self.branch_node_hash_masks.extend(other.branch_node_hash_masks);
        self.branch_node_tree_masks.extend(other.branch_node_tree_masks);

        for (hashed_address, storage) in other.storages {
            match self.storages.entry(hashed_address) {
                hash_map::Entry::Occupied(mut entry) => {
                    debug_assert_eq!(entry.get().root, storage.root);
                    let entry = entry.get_mut();
                    entry.subtree.extend_from(storage.subtree);
                    entry.branch_node_hash_masks.extend(storage.branch_node_hash_masks);
                    entry.branch_node_tree_masks.extend(storage.branch_node_tree_masks);
                }
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(storage);
                }
            }
        }
    }

    /// Create a [`MultiProof`] from a [`StorageMultiProof`].
    pub fn from_storage_proof(hashed_address: B256, storage_proof: StorageMultiProof) -> Self {
        Self {
            storages: B256Map::from_iter([(hashed_address, storage_proof)]),
            ..Default::default()
        }
    }
}

/// This is a type of [`MultiProof`] that uses decoded proofs, meaning these proofs are stored as a
/// collection of [`TrieNode`]s instead of RLP-encoded bytes.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct DecodedMultiProof {
    /// State trie multiproof for requested accounts.
    pub account_subtree: DecodedProofNodes,
    /// The hash masks of the branch nodes in the account proof.
    pub branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// The tree masks of the branch nodes in the account proof.
    pub branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    /// Storage trie multiproofs.
    pub storages: B256Map<DecodedStorageMultiProof>,
}

impl DecodedMultiProof {
    /// Returns true if the multiproof is empty.
    pub fn is_empty(&self) -> bool {
        self.account_subtree.is_empty() &&
            self.branch_node_hash_masks.is_empty() &&
            self.branch_node_tree_masks.is_empty() &&
            self.storages.is_empty()
    }

    /// Return the account proof nodes for the given account path.
    pub fn account_proof_nodes(&self, path: &Nibbles) -> Vec<(Nibbles, TrieNode)> {
        self.account_subtree.matching_nodes_sorted(path)
    }

    /// Return the storage proof nodes for the given storage slots of the account path.
    pub fn storage_proof_nodes(
        &self,
        hashed_address: B256,
        slots: impl IntoIterator<Item = B256>,
    ) -> Vec<(B256, Vec<(Nibbles, TrieNode)>)> {
        self.storages
            .get(&hashed_address)
            .map(|storage_mp| {
                slots
                    .into_iter()
                    .map(|slot| {
                        let nibbles = Nibbles::unpack(slot);
                        (slot, storage_mp.subtree.matching_nodes_sorted(&nibbles))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Construct the account proof from the multiproof.
    pub fn account_proof(
        &self,
        address: Address,
        slots: &[B256],
    ) -> Result<DecodedAccountProof, alloy_rlp::Error> {
        let hashed_address = keccak256(address);
        let nibbles = Nibbles::unpack(hashed_address);

        // Retrieve the account proof.
        let proof = self
            .account_proof_nodes(&nibbles)
            .into_iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();

        // Inspect the last node in the proof. If it's a leaf node with matching suffix,
        // then the node contains the encoded trie account.
        let info = 'info: {
            if let Some(TrieNode::Leaf(leaf)) = proof.last() {
                if nibbles.ends_with(&leaf.key) {
                    let account = TrieAccount::decode(&mut &leaf.value[..])?;
                    break 'info Some(Account {
                        balance: account.balance,
                        nonce: account.nonce,
                        bytecode_hash: (account.code_hash != KECCAK_EMPTY)
                            .then_some(account.code_hash),
                    })
                }
            }
            None
        };

        // Retrieve proofs for requested storage slots.
        let storage_multiproof = self.storages.get(&hashed_address);
        let storage_root = storage_multiproof.map(|m| m.root).unwrap_or(EMPTY_ROOT_HASH);
        let mut storage_proofs = Vec::with_capacity(slots.len());
        for slot in slots {
            let proof = if let Some(multiproof) = &storage_multiproof {
                multiproof.storage_proof(*slot)?
            } else {
                DecodedStorageProof::new(*slot)
            };
            storage_proofs.push(proof);
        }
        Ok(DecodedAccountProof { address, info, proof, storage_root, storage_proofs })
    }

    /// Extends this multiproof with another one, merging both account and storage
    /// proofs.
    pub fn extend(&mut self, other: Self) {
        self.account_subtree.extend_from(other.account_subtree);

        self.branch_node_hash_masks.extend(other.branch_node_hash_masks);
        self.branch_node_tree_masks.extend(other.branch_node_tree_masks);

        for (hashed_address, storage) in other.storages {
            match self.storages.entry(hashed_address) {
                hash_map::Entry::Occupied(mut entry) => {
                    debug_assert_eq!(entry.get().root, storage.root);
                    let entry = entry.get_mut();
                    entry.subtree.extend_from(storage.subtree);
                    entry.branch_node_hash_masks.extend(storage.branch_node_hash_masks);
                    entry.branch_node_tree_masks.extend(storage.branch_node_tree_masks);
                }
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(storage);
                }
            }
        }
    }

    /// Create a [`DecodedMultiProof`] from a [`DecodedStorageMultiProof`].
    pub fn from_storage_proof(
        hashed_address: B256,
        storage_proof: DecodedStorageMultiProof,
    ) -> Self {
        Self {
            storages: B256Map::from_iter([(hashed_address, storage_proof)]),
            ..Default::default()
        }
    }
}

impl TryFrom<MultiProof> for DecodedMultiProof {
    type Error = alloy_rlp::Error;

    fn try_from(multi_proof: MultiProof) -> Result<Self, Self::Error> {
        let account_subtree = DecodedProofNodes::try_from(multi_proof.account_subtree)?;
        let storages = multi_proof
            .storages
            .into_iter()
            .map(|(address, storage)| Ok((address, storage.try_into()?)))
            .collect::<Result<B256Map<_>, alloy_rlp::Error>>()?;
        Ok(Self {
            account_subtree,
            branch_node_hash_masks: multi_proof.branch_node_hash_masks,
            branch_node_tree_masks: multi_proof.branch_node_tree_masks,
            storages,
        })
    }
}

/// The merkle multiproof of storage trie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageMultiProof {
    /// Storage trie root.
    pub root: B256,
    /// Storage multiproof for requested slots.
    pub subtree: ProofNodes,
    /// The hash masks of the branch nodes in the storage proof.
    pub branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// The tree masks of the branch nodes in the storage proof.
    pub branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
}

impl StorageMultiProof {
    /// Create new storage multiproof for empty trie.
    pub fn empty() -> Self {
        Self {
            root: EMPTY_ROOT_HASH,
            subtree: ProofNodes::from_iter([(
                Nibbles::default(),
                Bytes::from([EMPTY_STRING_CODE]),
            )]),
            branch_node_hash_masks: HashMap::default(),
            branch_node_tree_masks: HashMap::default(),
        }
    }

    /// Return storage proofs for the target storage slot (unhashed).
    pub fn storage_proof(&self, slot: B256) -> Result<StorageProof, alloy_rlp::Error> {
        let nibbles = Nibbles::unpack(keccak256(slot));

        // Retrieve the storage proof.
        let proof = self
            .subtree
            .matching_nodes_iter(&nibbles)
            .sorted_by(|a, b| a.0.cmp(b.0))
            .map(|(_, node)| node.clone())
            .collect::<Vec<_>>();

        // Inspect the last node in the proof. If it's a leaf node with matching suffix,
        // then the node contains the encoded slot value.
        let value = 'value: {
            if let Some(last) = proof.last() {
                if let TrieNode::Leaf(leaf) = TrieNode::decode(&mut &last[..])? {
                    if nibbles.ends_with(&leaf.key) {
                        break 'value U256::decode(&mut &leaf.value[..])?
                    }
                }
            }
            U256::ZERO
        };

        Ok(StorageProof { key: slot, nibbles, value, proof })
    }
}

/// The decoded merkle multiproof for a storage trie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedStorageMultiProof {
    /// Storage trie root.
    pub root: B256,
    /// Storage multiproof for requested slots.
    pub subtree: DecodedProofNodes,
    /// The hash masks of the branch nodes in the storage proof.
    pub branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// The tree masks of the branch nodes in the storage proof.
    pub branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
}

impl DecodedStorageMultiProof {
    /// Create new storage multiproof for empty trie.
    pub fn empty() -> Self {
        Self {
            root: EMPTY_ROOT_HASH,
            subtree: DecodedProofNodes::from_iter([(Nibbles::default(), TrieNode::EmptyRoot)]),
            branch_node_hash_masks: HashMap::default(),
            branch_node_tree_masks: HashMap::default(),
        }
    }

    /// Return storage proofs for the target storage slot (unhashed).
    pub fn storage_proof(&self, slot: B256) -> Result<DecodedStorageProof, alloy_rlp::Error> {
        let nibbles = Nibbles::unpack(keccak256(slot));

        // Retrieve the storage proof.
        let proof = self
            .subtree
            .matching_nodes_iter(&nibbles)
            .sorted_by(|a, b| a.0.cmp(b.0))
            .map(|(_, node)| node.clone())
            .collect::<Vec<_>>();

        // Inspect the last node in the proof. If it's a leaf node with matching suffix,
        // then the node contains the encoded slot value.
        let value = 'value: {
            if let Some(TrieNode::Leaf(leaf)) = proof.last() {
                if nibbles.ends_with(&leaf.key) {
                    break 'value U256::decode(&mut &leaf.value[..])?
                }
            }
            U256::ZERO
        };

        Ok(DecodedStorageProof { key: slot, nibbles, value, proof })
    }
}

impl TryFrom<StorageMultiProof> for DecodedStorageMultiProof {
    type Error = alloy_rlp::Error;

    fn try_from(multi_proof: StorageMultiProof) -> Result<Self, Self::Error> {
        let subtree = DecodedProofNodes::try_from(multi_proof.subtree)?;
        Ok(Self {
            root: multi_proof.root,
            subtree,
            branch_node_hash_masks: multi_proof.branch_node_hash_masks,
            branch_node_tree_masks: multi_proof.branch_node_tree_masks,
        })
    }
}

/// The merkle proof with the relevant account info.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "serde"), serde(rename_all = "camelCase"))]
pub struct AccountProof {
    /// The address associated with the account.
    pub address: Address,
    /// Account info, if any.
    pub info: Option<Account>,
    /// Array of rlp-serialized merkle trie nodes which starting from the root node and
    /// following the path of the hashed address as key.
    pub proof: Vec<Bytes>,
    /// The storage trie root.
    pub storage_root: B256,
    /// Array of storage proofs as requested.
    pub storage_proofs: Vec<StorageProof>,
}

#[cfg(feature = "eip1186")]
impl AccountProof {
    /// Convert into an EIP-1186 account proof response
    pub fn into_eip1186_response(
        self,
        slots: Vec<alloy_serde::JsonStorageKey>,
    ) -> alloy_rpc_types_eth::EIP1186AccountProofResponse {
        let info = self.info.unwrap_or_default();
        alloy_rpc_types_eth::EIP1186AccountProofResponse {
            address: self.address,
            balance: info.balance,
            code_hash: info.get_bytecode_hash(),
            nonce: info.nonce,
            storage_hash: self.storage_root,
            account_proof: self.proof,
            storage_proof: self
                .storage_proofs
                .into_iter()
                .filter_map(|proof| {
                    let input_slot = slots.iter().find(|s| s.as_b256() == proof.key)?;
                    Some(proof.into_eip1186_proof(*input_slot))
                })
                .collect(),
        }
    }

    /// Converts an
    /// [`EIP1186AccountProofResponse`](alloy_rpc_types_eth::EIP1186AccountProofResponse) to an
    /// [`AccountProof`].
    ///
    /// This is the inverse of [`Self::into_eip1186_response`]
    pub fn from_eip1186_proof(proof: alloy_rpc_types_eth::EIP1186AccountProofResponse) -> Self {
        let alloy_rpc_types_eth::EIP1186AccountProofResponse {
            nonce,
            address,
            balance,
            code_hash,
            storage_hash,
            account_proof,
            storage_proof,
            ..
        } = proof;
        let storage_proofs = storage_proof.into_iter().map(Into::into).collect();

        let (storage_root, info) = if nonce == 0 &&
            balance.is_zero() &&
            storage_hash.is_zero() &&
            code_hash == KECCAK_EMPTY
        {
            // Account does not exist in state. Return `None` here to prevent proof
            // verification.
            (EMPTY_ROOT_HASH, None)
        } else {
            (storage_hash, Some(Account { nonce, balance, bytecode_hash: code_hash.into() }))
        };

        Self { address, info, proof: account_proof, storage_root, storage_proofs }
    }
}

#[cfg(feature = "eip1186")]
impl From<alloy_rpc_types_eth::EIP1186AccountProofResponse> for AccountProof {
    fn from(proof: alloy_rpc_types_eth::EIP1186AccountProofResponse) -> Self {
        Self::from_eip1186_proof(proof)
    }
}

impl Default for AccountProof {
    fn default() -> Self {
        Self::new(Address::default())
    }
}

impl AccountProof {
    /// Create new account proof entity.
    pub const fn new(address: Address) -> Self {
        Self {
            address,
            info: None,
            proof: Vec::new(),
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        }
    }

    /// Verify the storage proofs and account proof against the provided state root.
    #[expect(clippy::result_large_err)]
    pub fn verify(&self, root: B256) -> Result<(), ProofVerificationError> {
        // Verify storage proofs.
        for storage_proof in &self.storage_proofs {
            storage_proof.verify(self.storage_root)?;
        }

        // Verify the account proof.
        let expected = if self.info.is_none() && self.storage_root == EMPTY_ROOT_HASH {
            None
        } else {
            Some(alloy_rlp::encode(
                self.info.unwrap_or_default().into_trie_account(self.storage_root),
            ))
        };
        let nibbles = Nibbles::unpack(keccak256(self.address));
        verify_proof(root, nibbles, expected, &self.proof)
    }
}

/// The merkle proof with the relevant account info.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodedAccountProof {
    /// The address associated with the account.
    pub address: Address,
    /// Account info.
    pub info: Option<Account>,
    /// Array of merkle trie nodes which starting from the root node and following the path of the
    /// hashed address as key.
    pub proof: Vec<TrieNode>,
    /// The storage trie root.
    pub storage_root: B256,
    /// Array of storage proofs as requested.
    pub storage_proofs: Vec<DecodedStorageProof>,
}

impl Default for DecodedAccountProof {
    fn default() -> Self {
        Self::new(Address::default())
    }
}

impl DecodedAccountProof {
    /// Create new account proof entity.
    pub const fn new(address: Address) -> Self {
        Self {
            address,
            info: None,
            proof: Vec::new(),
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        }
    }
}

/// The merkle proof of the storage entry.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct StorageProof {
    /// The raw storage key.
    pub key: B256,
    /// The hashed storage key nibbles.
    pub nibbles: Nibbles,
    /// The storage value.
    pub value: U256,
    /// Array of rlp-serialized merkle trie nodes which starting from the storage root node and
    /// following the path of the hashed storage slot as key.
    pub proof: Vec<Bytes>,
}

impl StorageProof {
    /// Create new storage proof from the storage slot.
    pub fn new(key: B256) -> Self {
        let nibbles = Nibbles::unpack(keccak256(key));
        Self { key, nibbles, ..Default::default() }
    }

    /// Create new storage proof from the storage slot and its pre-hashed image.
    pub fn new_with_hashed(key: B256, hashed_key: B256) -> Self {
        Self { key, nibbles: Nibbles::unpack(hashed_key), ..Default::default() }
    }

    /// Create new storage proof from the storage slot and its pre-hashed image.
    pub fn new_with_nibbles(key: B256, nibbles: Nibbles) -> Self {
        Self { key, nibbles, ..Default::default() }
    }

    /// Set proof nodes on storage proof.
    pub fn with_proof(mut self, proof: Vec<Bytes>) -> Self {
        self.proof = proof;
        self
    }

    /// Verify the proof against the provided storage root.
    #[expect(clippy::result_large_err)]
    pub fn verify(&self, root: B256) -> Result<(), ProofVerificationError> {
        let expected =
            if self.value.is_zero() { None } else { Some(encode_fixed_size(&self.value).to_vec()) };
        verify_proof(root, self.nibbles.clone(), expected, &self.proof)
    }
}

#[cfg(feature = "eip1186")]
impl StorageProof {
    /// Convert into an EIP-1186 storage proof
    pub fn into_eip1186_proof(
        self,
        slot: alloy_serde::JsonStorageKey,
    ) -> alloy_rpc_types_eth::EIP1186StorageProof {
        alloy_rpc_types_eth::EIP1186StorageProof { key: slot, value: self.value, proof: self.proof }
    }

    /// Convert from an
    /// [`EIP1186StorageProof`](alloy_rpc_types_eth::EIP1186StorageProof)
    ///
    /// This is the inverse of [`Self::into_eip1186_proof`].
    pub fn from_eip1186_proof(storage_proof: alloy_rpc_types_eth::EIP1186StorageProof) -> Self {
        Self {
            value: storage_proof.value,
            proof: storage_proof.proof,
            ..Self::new(storage_proof.key.as_b256())
        }
    }
}

#[cfg(feature = "eip1186")]
impl From<alloy_rpc_types_eth::EIP1186StorageProof> for StorageProof {
    fn from(proof: alloy_rpc_types_eth::EIP1186StorageProof) -> Self {
        Self::from_eip1186_proof(proof)
    }
}

/// The merkle proof of the storage entry, using decoded proofs.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct DecodedStorageProof {
    /// The raw storage key.
    pub key: B256,
    /// The hashed storage key nibbles.
    pub nibbles: Nibbles,
    /// The storage value.
    pub value: U256,
    /// Array of merkle trie nodes which starting from the storage root node and following the path
    /// of the hashed storage slot as key.
    pub proof: Vec<TrieNode>,
}

impl DecodedStorageProof {
    /// Create new storage proof from the storage slot.
    pub fn new(key: B256) -> Self {
        let nibbles = Nibbles::unpack(keccak256(key));
        Self { key, nibbles, ..Default::default() }
    }

    /// Create new storage proof from the storage slot and its pre-hashed image.
    pub fn new_with_hashed(key: B256, hashed_key: B256) -> Self {
        Self { key, nibbles: Nibbles::unpack(hashed_key), ..Default::default() }
    }

    /// Create new storage proof from the storage slot and its pre-hashed image.
    pub fn new_with_nibbles(key: B256, nibbles: Nibbles) -> Self {
        Self { key, nibbles, ..Default::default() }
    }

    /// Set proof nodes on storage proof.
    pub fn with_proof(mut self, proof: Vec<TrieNode>) -> Self {
        self.proof = proof;
        self
    }
}

/// Implementation of hasher using our keccak256 hashing function
/// for compatibility with `triehash` crate.
#[cfg(any(test, feature = "test-utils"))]
pub mod triehash {
    use alloy_primitives::{keccak256, B256};
    use alloy_rlp::RlpEncodable;
    use hash_db::Hasher;
    use plain_hasher::PlainHasher;

    /// A [Hasher] that calculates a keccak256 hash of the given data.
    #[derive(Default, Debug, Clone, PartialEq, Eq, RlpEncodable)]
    #[non_exhaustive]
    pub struct KeccakHasher;

    #[cfg(any(test, feature = "test-utils"))]
    impl Hasher for KeccakHasher {
        type Out = B256;
        type StdHasher = PlainHasher;

        const LENGTH: usize = 32;

        fn hash(x: &[u8]) -> Self::Out {
            keccak256(x)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiproof_extend_account_proofs() {
        let mut proof1 = MultiProof::default();
        let mut proof2 = MultiProof::default();

        let addr1 = B256::random();
        let addr2 = B256::random();

        proof1.account_subtree.insert(
            Nibbles::unpack(addr1),
            alloy_rlp::encode_fixed_size(&U256::from(42)).to_vec().into(),
        );
        proof2.account_subtree.insert(
            Nibbles::unpack(addr2),
            alloy_rlp::encode_fixed_size(&U256::from(43)).to_vec().into(),
        );

        proof1.extend(proof2);

        assert!(proof1.account_subtree.contains_key(&Nibbles::unpack(addr1)));
        assert!(proof1.account_subtree.contains_key(&Nibbles::unpack(addr2)));
    }

    #[test]
    fn test_multiproof_extend_storage_proofs() {
        let mut proof1 = MultiProof::default();
        let mut proof2 = MultiProof::default();

        let addr = B256::random();
        let root = B256::random();

        let mut subtree1 = ProofNodes::default();
        subtree1.insert(
            Nibbles::from_nibbles(vec![0]),
            alloy_rlp::encode_fixed_size(&U256::from(42)).to_vec().into(),
        );
        proof1.storages.insert(
            addr,
            StorageMultiProof {
                root,
                subtree: subtree1,
                branch_node_hash_masks: HashMap::default(),
                branch_node_tree_masks: HashMap::default(),
            },
        );

        let mut subtree2 = ProofNodes::default();
        subtree2.insert(
            Nibbles::from_nibbles(vec![1]),
            alloy_rlp::encode_fixed_size(&U256::from(43)).to_vec().into(),
        );
        proof2.storages.insert(
            addr,
            StorageMultiProof {
                root,
                subtree: subtree2,
                branch_node_hash_masks: HashMap::default(),
                branch_node_tree_masks: HashMap::default(),
            },
        );

        proof1.extend(proof2);

        let storage = proof1.storages.get(&addr).unwrap();
        assert_eq!(storage.root, root);
        assert!(storage.subtree.contains_key(&Nibbles::from_nibbles(vec![0])));
        assert!(storage.subtree.contains_key(&Nibbles::from_nibbles(vec![1])));
    }

    #[test]
    fn test_multi_proof_retain_difference() {
        let mut empty = MultiProofTargets::default();
        empty.retain_difference(&Default::default());
        assert!(empty.is_empty());

        let targets = MultiProofTargets::accounts((0..10).map(B256::with_last_byte));

        let mut diffed = targets.clone();
        diffed.retain_difference(&MultiProofTargets::account(B256::with_last_byte(11)));
        assert_eq!(diffed, targets);

        diffed.retain_difference(&MultiProofTargets::accounts((0..5).map(B256::with_last_byte)));
        assert_eq!(diffed, MultiProofTargets::accounts((5..10).map(B256::with_last_byte)));

        diffed.retain_difference(&targets);
        assert!(diffed.is_empty());

        let mut targets = MultiProofTargets::default();
        let (account1, account2, account3) =
            (1..=3).map(B256::with_last_byte).collect_tuple().unwrap();
        let account2_slots = (1..5).map(B256::with_last_byte).collect::<B256Set>();
        targets.insert(account1, B256Set::from_iter([B256::with_last_byte(1)]));
        targets.insert(account2, account2_slots.clone());
        targets.insert(account3, B256Set::from_iter([B256::with_last_byte(1)]));

        let mut diffed = targets.clone();
        diffed.retain_difference(&MultiProofTargets::accounts((1..=3).map(B256::with_last_byte)));
        assert_eq!(diffed, targets);

        // remove last 3 slots for account 2
        let mut account2_slots_expected_len = account2_slots.len();
        for slot in account2_slots.iter().skip(1) {
            diffed.retain_difference(&MultiProofTargets::account_with_slots(account2, [*slot]));
            account2_slots_expected_len -= 1;
            assert_eq!(
                diffed.get(&account2).map(|slots| slots.len()),
                Some(account2_slots_expected_len)
            );
        }

        diffed.retain_difference(&targets);
        assert!(diffed.is_empty());
    }

    #[test]
    fn test_multi_proof_retain_difference_no_overlap() {
        let mut targets = MultiProofTargets::default();

        // populate some targets
        let (addr1, addr2) = (B256::random(), B256::random());
        let (slot1, slot2) = (B256::random(), B256::random());
        targets.insert(addr1, vec![slot1].into_iter().collect());
        targets.insert(addr2, vec![slot2].into_iter().collect());

        let mut retained = targets.clone();
        retained.retain_difference(&Default::default());
        assert_eq!(retained, targets);

        // add a different addr and slot to fetched proof targets
        let mut other_targets = MultiProofTargets::default();
        let addr3 = B256::random();
        let slot3 = B256::random();
        other_targets.insert(addr3, B256Set::from_iter([slot3]));

        // check that the prefetch proof targets are the same because the fetched proof targets
        // don't overlap with the prefetch targets
        let mut retained = targets.clone();
        retained.retain_difference(&other_targets);
        assert_eq!(retained, targets);
    }

    #[test]
    fn test_get_prefetch_proof_targets_remove_subset() {
        // populate some targets
        let mut targets = MultiProofTargets::default();
        let (addr1, addr2) = (B256::random(), B256::random());
        let (slot1, slot2) = (B256::random(), B256::random());
        targets.insert(addr1, B256Set::from_iter([slot1]));
        targets.insert(addr2, B256Set::from_iter([slot2]));

        // add a subset of the first target to other proof targets
        let other_targets = MultiProofTargets::account_with_slots(addr1, [slot1]);

        let mut retained = targets.clone();
        retained.retain_difference(&other_targets);

        // check that the prefetch proof targets do not include the subset
        assert_eq!(retained.len(), 1);
        assert!(!retained.contains_key(&addr1));
        assert!(retained.contains_key(&addr2));

        // now add one more slot to the prefetch targets
        let slot3 = B256::random();
        targets.get_mut(&addr1).unwrap().insert(slot3);

        let mut retained = targets.clone();
        retained.retain_difference(&other_targets);

        // check that the prefetch proof targets do not include the subset
        // but include the new slot
        assert_eq!(retained.len(), 2);
        assert!(retained.contains_key(&addr1));
        assert_eq!(retained.get(&addr1), Some(&B256Set::from_iter([slot3])));
        assert!(retained.contains_key(&addr2));
        assert_eq!(retained.get(&addr2), Some(&B256Set::from_iter([slot2])));
    }

    #[test]
    #[cfg(feature = "eip1186")]
    fn eip_1186_roundtrip() {
        let mut acc = AccountProof {
            address: Address::random(),
            info: Some(
                // non-empty account
                Account { nonce: 100, balance: U256::ZERO, bytecode_hash: Some(KECCAK_EMPTY) },
            ),
            proof: vec![],
            storage_root: B256::ZERO,
            storage_proofs: vec![],
        };

        let rpc_proof = acc.clone().into_eip1186_response(Vec::new());
        let inverse: AccountProof = rpc_proof.into();
        assert_eq!(acc, inverse);

        // make account empty
        acc.info.as_mut().unwrap().nonce = 0;
        let rpc_proof = acc.clone().into_eip1186_response(Vec::new());
        let inverse: AccountProof = rpc_proof.into();
        acc.info.take();
        acc.storage_root = EMPTY_ROOT_HASH;
        assert_eq!(acc, inverse);
    }
}
