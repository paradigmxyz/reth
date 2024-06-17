//! Merkle trie proofs.

use crate::{Nibbles, TrieAccount};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::encode_fixed_size;
use alloy_trie::{
    proof::{verify_proof, ProofVerificationError},
    EMPTY_ROOT_HASH,
};
use reth_primitives_traits::Account;

/// The merkle proof with the relevant account info.
#[derive(PartialEq, Eq, Debug)]
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
    pub const fn new(address: Address) -> Self {
        Self {
            address,
            info: None,
            proof: Vec::new(),
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        }
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
