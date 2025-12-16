//! Consensus types for Era post-merge history files
//!
//! NOTE: Decompression is bounded to avoid unbounded memory allocation on
//! malformed or malicious ERA inputs.
//!
//! # Decoding
//!
//! This crate only handles compression/decompression.
//! To decode the SSZ data into concrete beacon types, use the [Lighthouse `types`](https://github.com/sigp/lighthouse/tree/stable/consensus/types)
//! crate or another SSZ-compatible library.

use crate::e2s::{error::E2sError, types::Entry};
use snap::{read::FrameDecoder, write::FrameEncoder};
use std::io::{Read, Write};

/// Maximum allowed decompressed size for a signed beacon block SSZ payload.
///
/// This is a safety bound to prevent unbounded memory allocation on malformed inputs.
const MAX_DECOMPRESSED_SIGNED_BEACON_BLOCK_BYTES: usize = 256 * 1024 * 1024; // 256 MiB

/// Maximum allowed decompressed size for a beacon state SSZ payload.
///
/// Beacon states are much larger than blocks, but still need a hard ceiling.
const MAX_DECOMPRESSED_BEACON_STATE_BYTES: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

fn decompress_snappy_bounded(
    compressed: &[u8],
    max_decompressed_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, E2sError> {
    let mut decoder = FrameDecoder::new(compressed).take(max_decompressed_bytes as u64);
    let mut decompressed = Vec::new();

    Read::read_to_end(&mut decoder, &mut decompressed)
        .map_err(|e| E2sError::SnappyDecompression(format!("Failed to decompress {what}: {e}")))?;

    // Fail closed: if we hit the cap exactly, treat it as limit exceeded.
    if decompressed.len() >= max_decompressed_bytes {
        return Err(E2sError::SnappyDecompression(format!(
            "Failed to decompress {what}: decompressed data exceeded limit of {max_decompressed_bytes} bytes"
        )));
    }

    Ok(decompressed)
}

/// `CompressedSignedBeaconBlock` record type: [0x01, 0x00]
pub const COMPRESSED_SIGNED_BEACON_BLOCK: [u8; 2] = [0x01, 0x00];

/// `CompressedBeaconState` record type: [0x02, 0x00]
pub const COMPRESSED_BEACON_STATE: [u8; 2] = [0x02, 0x00];

/// Compressed signed beacon block
#[derive(Debug, Clone)]
pub struct CompressedSignedBeaconBlock {
    /// Snappy-compressed ssz-encoded `SignedBeaconBlock`
    pub data: Vec<u8>,
}

impl CompressedSignedBeaconBlock {
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

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

    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        decompress_snappy_bounded(
            self.data.as_slice(),
            MAX_DECOMPRESSED_SIGNED_BEACON_BLOCK_BYTES,
            "signed beacon block",
        )
    }

    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_SIGNED_BEACON_BLOCK, self.data.clone())
    }

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
}

/// Compressed beacon state
#[derive(Debug, Clone)]
pub struct CompressedBeaconState {
    /// Snappy-compressed ssz-encoded `BeaconState`
    pub data: Vec<u8>,
}

impl CompressedBeaconState {
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

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

    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        decompress_snappy_bounded(
            self.data.as_slice(),
            MAX_DECOMPRESSED_BEACON_STATE_BYTES,
            "beacon state",
        )
    }

    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_BEACON_STATE, self.data.clone())
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signed_beacon_block_compression_roundtrip() {
        let ssz_data = vec![1, 2, 3, 4, 5];
        let compressed = CompressedSignedBeaconBlock::from_ssz(&ssz_data).unwrap();
        let decompressed = compressed.decompress().unwrap();
        assert_eq!(decompressed, ssz_data);
    }

    #[test]
    fn test_beacon_state_compression_roundtrip() {
        let ssz_data = vec![5, 4, 3, 2, 1];
        let compressed = CompressedBeaconState::from_ssz(&ssz_data).unwrap();
        let decompressed = compressed.decompress().unwrap();
        assert_eq!(decompressed, ssz_data);
    }

    #[test]
    fn test_bounded_decompression_rejects_oversized_output() {
        let ssz_data = vec![42u8; 1024];
        let compressed = CompressedBeaconState::from_ssz(&ssz_data).unwrap();

        let err = decompress_snappy_bounded(
            compressed.data.as_slice(),
            100, // intentionally tiny limit
            "beacon state",
        )
        .unwrap_err();

        let msg = format!("{err:?}");
        assert!(msg.contains("exceeded limit"));
    }
}
