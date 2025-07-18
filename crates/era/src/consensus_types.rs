//! Consensus types for Era post-merge history files

use crate::{
    e2s_types::{E2sError, Entry},
    DecodeCompressedSsz,
};
use snap::{read::FrameDecoder, write::FrameEncoder};
use ssz::Decode;
use std::io::{Read, Write};

/// `CompressedSignedBeaconBlock` record type: [0x01, 0x00]
pub const COMPRESSED_SIGNED_BEACON_BLOCK: [u8; 2] = [0x01, 0x00];

/// `CompressedBeaconState` record type: [0x02, 0x00]
pub const COMPRESSED_BEACON_STATE: [u8; 2] = [0x02, 0x00];

/// Compressed signed beacon block
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedsignedbeaconblock>.
#[derive(Debug, Clone)]
pub struct CompressedSignedBeaconBlock {
    /// Snappy-compressed ssz-encoded `SignedBeaconBlock`
    pub data: Vec<u8>,
}

impl CompressedSignedBeaconBlock {
    /// Create a new [`CompressedSignedBeaconBlock`] from compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from ssz-encoded block by compressing it with snappy
    pub fn from_ssz(ssz_data: &[u8]) -> Result<Self, E2sError> {
        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, ssz_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress signed beacon block: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }
        Ok(Self { data: compressed })
    }

    /// Decompress to get the original ssz-encoded signed beacon block
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = FrameDecoder::new(self.data.as_slice());
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress signed beacon block: {e}"))
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
                COMPRESSED_SIGNED_BEACON_BLOCK[0],
                COMPRESSED_SIGNED_BEACON_BLOCK[1],
                entry.entry_type[0],
                entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }

    /// Decode the compressed signed beacon block into ssz bytes
    pub fn decode_to_ssz(&self) -> Result<Vec<u8>, E2sError> {
        self.decompress()
    }
}

impl DecodeCompressedSsz for CompressedSignedBeaconBlock {
    fn decode<T: Decode>(&self) -> Result<T, E2sError> {
        let ssz_bytes = self.decompress()?;
        T::from_ssz_bytes(&ssz_bytes).map_err(|e| {
            E2sError::Ssz(format!("Failed to decode SSZ data into target type: {e:?}"))
        })
    }
}

/// Compressed beacon state
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedbeaconstate>.
#[derive(Debug, Clone)]
pub struct CompressedBeaconState {
    /// Snappy-compressed ssz-encoded `BeaconState`
    pub data: Vec<u8>,
}

impl CompressedBeaconState {
    /// Create a new [`CompressedBeaconState`] from compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Compress with snappy from ssz-encoded state
    pub fn from_ssz(ssz_data: &[u8]) -> Result<Self, E2sError> {
        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, ssz_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress beacon state: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }
        Ok(Self { data: compressed })
    }

    /// Decompress to get the original ssz-encoded beacon state
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = FrameDecoder::new(self.data.as_slice());
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress beacon state: {e}"))
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
                COMPRESSED_BEACON_STATE[0],
                COMPRESSED_BEACON_STATE[1],
                entry.entry_type[0],
                entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }

    /// Decode the compressed beacon state into ssz bytes
    pub fn decode_to_ssz(&self) -> Result<Vec<u8>, E2sError> {
        self.decompress()
    }
}

impl DecodeCompressedSsz for CompressedBeaconState {
    fn decode<T: Decode>(&self) -> Result<T, E2sError> {
        let ssz_bytes = self.decompress()?;
        T::from_ssz_bytes(&ssz_bytes).map_err(|e| {
            E2sError::Ssz(format!("Failed to decode SSZ data into target type: {e:?}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signed_beacon_block_compression_roundtrip() {
        let ssz_data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        let compressed_block = CompressedSignedBeaconBlock::from_ssz(&ssz_data).unwrap();
        let decompressed = compressed_block.decompress().unwrap();

        assert_eq!(decompressed, ssz_data);
    }

    #[test]
    fn test_beacon_state_compression_roundtrip() {
        let ssz_data = vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1];

        let compressed_state = CompressedBeaconState::from_ssz(&ssz_data).unwrap();
        let decompressed = compressed_state.decompress().unwrap();

        assert_eq!(decompressed, ssz_data);
    }

    #[test]
    fn test_entry_conversion_signed_beacon_block() {
        let ssz_data = vec![1, 2, 3, 4, 5];
        let compressed_block = CompressedSignedBeaconBlock::from_ssz(&ssz_data).unwrap();

        let entry = compressed_block.to_entry();
        assert_eq!(entry.entry_type, COMPRESSED_SIGNED_BEACON_BLOCK);

        let recovered = CompressedSignedBeaconBlock::from_entry(&entry).unwrap();
        let recovered_ssz = recovered.decode_to_ssz().unwrap();

        assert_eq!(recovered_ssz, ssz_data);
    }

    #[test]
    fn test_entry_conversion_beacon_state() {
        let ssz_data = vec![5, 4, 3, 2, 1];
        let compressed_state = CompressedBeaconState::from_ssz(&ssz_data).unwrap();

        let entry = compressed_state.to_entry();
        assert_eq!(entry.entry_type, COMPRESSED_BEACON_STATE);

        let recovered = CompressedBeaconState::from_entry(&entry).unwrap();
        let recovered_ssz = recovered.decode_to_ssz().unwrap();

        assert_eq!(recovered_ssz, ssz_data);
    }

    #[test]
    fn test_invalid_entry_type() {
        let invalid_entry = Entry::new([0xFF, 0xFF], vec![1, 2, 3]);

        let result = CompressedSignedBeaconBlock::from_entry(&invalid_entry);
        assert!(result.is_err());

        let result = CompressedBeaconState::from_entry(&invalid_entry);
        assert!(result.is_err());
    }
}
