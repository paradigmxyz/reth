//! Data models for external proofs storage

use alloy_primitives::B256;
use reth_db_api::{
    table::{Decode, Encode},
    DatabaseError,
};
use serde::{Deserialize, Serialize};

// Re-export for convenience
pub use reth_db_api::models::IntegerList;

/// Composite key for (BlockNumber, HashedAddress)
///
/// Similar to `BlockNumberAddress` but uses B256 for hashed addresses.
/// Used for indexing by block number and hashed address in external storage.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct BlockNumberHashedAddress(pub (u64, B256));

impl BlockNumberHashedAddress {
    /// Create a new instance
    pub const fn new(block_number: u64, hashed_address: B256) -> Self {
        Self((block_number, hashed_address))
    }

    /// Get the block number
    pub const fn block_number(&self) -> u64 {
        self.0 .0
    }

    /// Get the hashed address
    pub const fn hashed_address(&self) -> B256 {
        self.0 .1
    }

    /// Destructure into components
    pub const fn into_components(self) -> (u64, B256) {
        self.0
    }
}

impl From<(u64, B256)> for BlockNumberHashedAddress {
    fn from(tuple: (u64, B256)) -> Self {
        Self(tuple)
    }
}

impl Encode for BlockNumberHashedAddress {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let (block_number, hashed_address) = self.0;

        // Use big-endian encoding for block number to ensure lexicographic = numeric ordering
        let block_bytes = block_number.to_be_bytes();
        let addr_bytes = hashed_address.as_slice();

        let mut buf = Vec::with_capacity(8 + 32);
        buf.extend_from_slice(&block_bytes);
        buf.extend_from_slice(addr_bytes);
        buf
    }
}

impl Decode for BlockNumberHashedAddress {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != 40 {
            // 8 bytes for block number + 32 bytes for B256
            return Err(DatabaseError::Decode);
        }

        let block_number =
            u64::from_be_bytes(value[..8].try_into().map_err(|_| DatabaseError::Decode)?);

        let hashed_address = B256::from_slice(&value[8..40]);

        Ok(Self((block_number, hashed_address)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_number_hashed_address_encode_decode() {
        let key = BlockNumberHashedAddress::new(42, B256::repeat_byte(0xaa));
        let encoded = key.encode();
        let decoded = BlockNumberHashedAddress::decode(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn test_block_number_hashed_address_ordering() {
        let key1 = BlockNumberHashedAddress::new(1, B256::repeat_byte(0xaa));
        let key2 = BlockNumberHashedAddress::new(2, B256::repeat_byte(0xaa));
        let key3 = BlockNumberHashedAddress::new(2, B256::repeat_byte(0xbb));

        // Encoded bytes should be sortable by block number first, then address
        let enc1 = key1.encode();
        let enc2 = key2.encode();
        let enc3 = key3.encode();

        assert!(enc1 < enc2);
        assert!(enc2 < enc3);
    }

    #[test]
    fn test_block_number_hashed_address_accessors() {
        let key = BlockNumberHashedAddress::new(42, B256::repeat_byte(0xaa));
        assert_eq!(key.block_number(), 42);
        assert_eq!(key.hashed_address(), B256::repeat_byte(0xaa));

        let (block, addr) = key.into_components();
        assert_eq!(block, 42);
        assert_eq!(addr, B256::repeat_byte(0xaa));
    }
}
