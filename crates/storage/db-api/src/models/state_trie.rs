//! Keys for complete storage tries in `RocksDB`.
use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::B256;
use reth_trie_common::PackedStoredNibbles;
use serde::{Deserialize, Serialize};

/// Account hash followed by a fixed-width packed trie path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StateTrieStorageKey {
    /// Account's hashed address.
    pub address: B256,
    /// Path within the account's storage trie.
    pub path: PackedStoredNibbles,
}

impl Encode for StateTrieStorageKey {
    type Encoded = [u8; 65];

    fn encode(self) -> Self::Encoded {
        let mut key = [0; 65];
        key[..32].copy_from_slice(self.address.as_slice());
        key[32..].copy_from_slice(self.path.encode().as_ref());
        key
    }
}

impl Decode for StateTrieStorageKey {
    fn decode(key: &[u8]) -> Result<Self, DatabaseError> {
        if key.len() != 65 {
            return Err(DatabaseError::Decode)
        }
        Ok(Self {
            address: B256::from_slice(&key[..32]),
            path: PackedStoredNibbles::decode(&key[32..])?,
        })
    }
}
