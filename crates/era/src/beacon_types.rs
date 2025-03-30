//! Beacon chain specific types for e2store files
//!
//! Contains implementations for compressed beacon chain data structures:
//! - [`CompressedSignedBeaconBlock`]
//! - [`CompressedBeaconState`]
//!
//! These types use Snappy compression to match the p2p specification.
//!
//! See also
//! <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedsignedbeaconblock>
//! and <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedbeaconstate>

use crate::e2s_types::{E2sError, Entry, COMPRESSED_BEACON_STATE, COMPRESSED_SIGNED_BEACON_BLOCK};
use snap::raw::{Decoder, Encoder};

/// [`CompressedSignedBeaconBlock`] contains a `SignedBeaconBlock` compressed using Snappy
#[derive(Debug, Clone)]
pub struct CompressedSignedBeaconBlock {
    /// The compressed data, using `snappy(ssz(SignedBeaconBlock))` format
    pub data: Vec<u8>,
}

impl CompressedSignedBeaconBlock {
    /// Create a new [`CompressedSignedBeaconBlock`] from compressed data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from an SSZ-encoded `SignedBeaconBlock` by compressing it
    pub fn from_ssz(ssz_data: &[u8]) -> Result<Self, E2sError> {
        let mut encoder = Encoder::new();
        let compressed = encoder
            .compress_vec(ssz_data)
            .map_err(|e| E2sError::SnappyCompression(format!("Failed to compress data: {}", e)))?;

        Ok(Self { data: compressed })
    }

    /// Decompress to get the original SSZ-encoded `SignedBeaconBlock`
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(&self.data).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress data: {}", e))
        })?;

        Ok(decompressed)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_SIGNED_BEACON_BLOCK, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != COMPRESSED_SIGNED_BEACON_BLOCK {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for CompressedSignedBeaconBlock: expected {:02x}{:02x}, got {:02x}{:02x}",
                COMPRESSED_SIGNED_BEACON_BLOCK[0], COMPRESSED_SIGNED_BEACON_BLOCK[1],
                entry.entry_type[0], entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }
}

/// [`CompressedBeaconState`] contains a `BeaconState` compressed using Snappy
#[derive(Debug, Clone)]
pub struct CompressedBeaconState {
    /// The compressed data, using `snappy(ssz(BeaconState))` format
    pub data: Vec<u8>,
}

impl CompressedBeaconState {
    /// Create a new [`CompressedBeaconState`] from compressed data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from an SSZ-encoded `BeaconState` by compressing it
    pub fn from_ssz(ssz_data: &[u8]) -> Result<Self, E2sError> {
        let mut encoder = Encoder::new();
        let compressed = encoder
            .compress_vec(ssz_data)
            .map_err(|e| E2sError::SnappyCompression(format!("Failed to compress data: {}", e)))?;

        Ok(Self { data: compressed })
    }

    /// Decompress to get the original SSZ-encoded `BeaconState`
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(&self.data).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress data: {}", e))
        })?;

        Ok(decompressed)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_BEACON_STATE, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != COMPRESSED_BEACON_STATE {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for CompressedBeaconState: expected {:02x}{:02x}, got {:02x}{:02x}",
                COMPRESSED_BEACON_STATE[0], COMPRESSED_BEACON_STATE[1],
                entry.entry_type[0], entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        beacon_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
        e2s_types::{COMPRESSED_BEACON_STATE, COMPRESSED_SIGNED_BEACON_BLOCK},
    };

    #[test]
    fn test_compressed_signed_beacon_block() {
        let original_data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        // Test full round trip: SSZ --> compress --> entry --> parse --> decompress
        let compressed_block = CompressedSignedBeaconBlock::from_ssz(&original_data).unwrap();
        let entry = compressed_block.to_entry();
        assert_eq!(entry.entry_type, COMPRESSED_SIGNED_BEACON_BLOCK);

        let recovered_block = CompressedSignedBeaconBlock::from_entry(&entry).unwrap();
        let decompressed_data = recovered_block.decompress().unwrap();

        // Verify the round trip was successful
        assert_eq!(decompressed_data, original_data);
    }

    #[test]
    fn test_compressed_beacon_state() {
        let original_data = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

        // Test full round trip: SSZ --> compress --> entry --> parse --> decompress
        let compressed_state = CompressedBeaconState::from_ssz(&original_data).unwrap();
        let entry = compressed_state.to_entry();
        assert_eq!(entry.entry_type, COMPRESSED_BEACON_STATE);

        let recovered_state = CompressedBeaconState::from_entry(&entry).unwrap();
        let decompressed_data = recovered_state.decompress().unwrap();

        // Verify the round trip was successful
        assert_eq!(decompressed_data, original_data);
    }
}
