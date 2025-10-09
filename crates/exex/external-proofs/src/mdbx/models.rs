//! Custom data models for MDBX external storage
//!
//! This module defines composite key types used for efficient indexing in MDBX tables.

use alloy_primitives::B256;
use reth_db_api::{
    table::{Decode, Encode},
    DatabaseError,
};
use reth_trie::Nibbles;
use reth_trie_common::StoredNibbles;
use serde::{Deserialize, Serialize};

/// Composite key: (block_number, path)
///
/// Used for indexing trie branches by block number and path.
/// The block number is encoded in big-endian to ensure lexicographic
/// ordering matches numeric ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockPath(pub u64, pub StoredNibbles);

impl Encode for BlockPath {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        // Use big-endian encoding for block number to ensure lexicographic = numeric ordering
        let block_bytes = self.0.to_be_bytes();
        let nibbles_bytes: Vec<u8> = self.1.encode();

        let mut buf = Vec::with_capacity(8 + nibbles_bytes.len());
        buf.extend_from_slice(&block_bytes);
        buf.extend_from_slice(&nibbles_bytes);
        buf
    }
}

impl Decode for BlockPath {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() < 8 {
            return Err(DatabaseError::Decode);
        }

        let block = u64::from_be_bytes(value[..8].try_into().map_err(|_| DatabaseError::Decode)?);
        let nibbles = StoredNibbles::decode(&value[8..])?;
        Ok(Self(block, nibbles))
    }
}

impl From<(u64, Nibbles)> for BlockPath {
    fn from((block, nibbles): (u64, Nibbles)) -> Self {
        Self(block, StoredNibbles(nibbles))
    }
}

/// Composite subkey for storage branches: (path, block_number)
///
/// Used for storage trie branches indexed by address. The path comes first
/// so we can iterate by path, then block number gives us versioning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StorageBranchSubKey(pub StoredNibbles, pub u64);

impl Encode for StorageBranchSubKey {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let nibbles_bytes: Vec<u8> = self.0.encode();
        let block_bytes = self.1.to_be_bytes();

        let mut buf = Vec::with_capacity(nibbles_bytes.len() + 8);
        buf.extend_from_slice(&nibbles_bytes);
        buf.extend_from_slice(&block_bytes);
        buf
    }
}

impl Decode for StorageBranchSubKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() < 8 {
            return Err(DatabaseError::Decode);
        }

        let block = u64::from_be_bytes(
            value[value.len() - 8..].try_into().map_err(|_| DatabaseError::Decode)?,
        );
        let nibbles = StoredNibbles::decode(&value[..value.len() - 8])?;
        Ok(Self(nibbles, block))
    }
}

/// Composite subkey for hashed storage: (storage_key, block_number)
///
/// Used for storage values indexed by hashed address. The storage key comes first
/// so we can iterate by storage key, then block number gives us versioning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HashedStorageSubKey(pub B256, pub u64);

impl Encode for HashedStorageSubKey {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let mut buf = Vec::with_capacity(32 + 8);
        buf.extend_from_slice(self.0.as_slice());
        buf.extend_from_slice(&self.1.to_be_bytes());
        buf
    }
}

impl Decode for HashedStorageSubKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != 40 {
            return Err(DatabaseError::Decode);
        }

        let storage_key = B256::from_slice(&value[..32]);
        let block =
            u64::from_be_bytes(value[32..40].try_into().map_err(|_| DatabaseError::Decode)?);
        Ok(Self(storage_key, block))
    }
}

/// Metadata keys for tracking block ranges
///
/// Used to store earliest and latest block numbers in the external storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MetadataKey {
    /// Earliest block number stored in external storage
    EarliestBlock = 0,
    /// Latest block number stored in external storage
    LatestBlock = 1,
}

impl Encode for MetadataKey {
    type Encoded = [u8; 1];

    fn encode(self) -> Self::Encoded {
        [self as u8]
    }
}

impl Decode for MetadataKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        match value.first() {
            Some(&0) => Ok(Self::EarliestBlock),
            Some(&1) => Ok(Self::LatestBlock),
            _ => Err(DatabaseError::Decode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_path_encode_decode() {
        let path = BlockPath(42, StoredNibbles(Nibbles::from_nibbles_unchecked(&[1, 2, 3, 4])));
        let encoded = path.clone().encode();
        let decoded = BlockPath::decode(&encoded).unwrap();
        assert_eq!(path, decoded);
    }

    #[test]
    fn test_block_path_ordering() {
        let path1 = BlockPath(1, StoredNibbles(Nibbles::from_nibbles_unchecked(&[1, 2])));
        let path2 = BlockPath(2, StoredNibbles(Nibbles::from_nibbles_unchecked(&[1, 2])));
        let path3 = BlockPath(2, StoredNibbles(Nibbles::from_nibbles_unchecked(&[1, 3])));

        // Encoded bytes should be sortable
        let enc1 = path1.encode();
        let enc2 = path2.encode();
        let enc3 = path3.encode();

        assert!(enc1 < enc2);
        assert!(enc2 < enc3);
    }

    #[test]
    fn test_metadata_key_encode_decode() {
        let key = MetadataKey::EarliestBlock;
        let encoded = key.encode();
        let decoded = MetadataKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);

        let key = MetadataKey::LatestBlock;
        let encoded = key.encode();
        let decoded = MetadataKey::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }
}
