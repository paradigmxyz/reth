//! Merkle trie proofs.

use crate::{Nibbles, TrieAccount};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::{encode_fixed_size, Decodable, EMPTY_STRING_CODE};
use alloy_trie::{
    nodes::TrieNode,
    proof::{verify_proof, ProofNodes, ProofVerificationError},
    EMPTY_ROOT_HASH,
};
use itertools::Itertools;
use reth_primitives_traits::{constants::KECCAK_EMPTY, Account};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The state multiproof of target accounts and multiproofs of their storage tries.
/// Multiproof is effectively a state subtrie that only contains the nodes
/// in the paths of target accounts.  
#[derive(Clone, Default, Debug)]
pub struct MultiProof {
    /// State trie multiproof for requested accounts.
    pub account_subtree: ProofNodes,
    /// Storage trie multiproofs.
    pub storages: HashMap<B256, StorageMultiProof>,
}

impl MultiProof {
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
            .account_subtree
            .matching_nodes_iter(&nibbles)
            .sorted_by(|a, b| a.0.cmp(b.0))
            .map(|(_, node)| node.clone())
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
}

/// The merkle multiproof of storage trie.
#[derive(Clone, Debug)]
pub struct StorageMultiProof {
    /// Storage trie root.
    pub root: B256,
    /// Storage multiproof for requested slots.
    pub subtree: ProofNodes,
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

/// The merkle proof with the relevant account info.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProof {
    /// The address associated with the account.
    pub address: Address,
    /// Account info.
    pub info: Option<Account>,
    /// Array of rlp-serialized merkle trie nodes which starting from the root node and
    /// following the path of the hashed address as key.
    pub proof: Vec<Bytes>,
    /// The storage trie root.
    pub storage_root: B256,
    /// Array of storage proofs as requested.
    pub storage_proofs: Vec<StorageProof>,
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
    pub fn verify(&self, root: B256) -> Result<(), ProofVerificationError> {
        // Verify storage proofs.
        for storage_proof in &self.storage_proofs {
            storage_proof.verify(self.storage_root)?;
        }

        // Verify the account proof.
        let expected = if self.info.is_none() && self.storage_root == EMPTY_ROOT_HASH {
            None
        } else {
            Some(alloy_rlp::encode(TrieAccount::from((
                self.info.unwrap_or_default(),
                self.storage_root,
            ))))
        };
        let nibbles = Nibbles::unpack(keccak256(self.address));
        verify_proof(root, nibbles, expected, &self.proof)
    }
}

/// The merkle proof of the storage entry.
#[derive(Clone, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
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
    pub fn verify(&self, root: B256) -> Result<(), ProofVerificationError> {
        let expected =
            if self.value.is_zero() { None } else { Some(encode_fixed_size(&self.value).to_vec()) };
        verify_proof(root, self.nibbles.clone(), expected, &self.proof)
    }
}

/// Implementation of hasher using our keccak256 hashing function
/// for compatibility with `triehash` crate.
#[cfg(any(test, feature = "test-utils"))]
pub mod triehash {
    use alloy_primitives::{keccak256, B256};
    use hash_db::Hasher;
    use plain_hasher::PlainHasher;

    /// A [Hasher] that calculates a keccak256 hash of the given data.
    #[derive(Default, Debug, Clone, PartialEq, Eq)]
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
