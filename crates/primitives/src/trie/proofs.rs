//! Merkle trie proofs.

use super::Nibbles;
use crate::{keccak256, Account, Address, Bytes, B256, U256};

/// The merkle proof with the relevant account info.
#[derive(PartialEq, Eq, Default, Debug)]
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

impl AccountProof {
    /// Create new account proof entity.
    pub fn new(address: Address) -> Self {
        Self { address, ..Default::default() }
    }

    /// Set account info, storage root and requested storage proofs.
    pub fn set_account(
        &mut self,
        info: Account,
        storage_root: B256,
        storage_proofs: Vec<StorageProof>,
    ) {
        self.info = Some(info);
        self.storage_root = storage_root;
        self.storage_proofs = storage_proofs;
    }

    /// Set proof path.
    pub fn set_proof(&mut self, proof: Vec<Bytes>) {
        self.proof = proof;
    }
}

/// The merkle proof of the storage entry.
#[derive(PartialEq, Eq, Default, Debug)]
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

    /// Set storage value.
    pub fn set_value(&mut self, value: U256) {
        self.value = value;
    }

    /// Set proof path.
    pub fn set_proof(&mut self, proof: Vec<Bytes>) {
        self.proof = proof;
    }
}
