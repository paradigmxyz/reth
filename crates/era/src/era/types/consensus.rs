//! Consensus types for Era post-merge history files
//!
//! # Decoding
//!
//! This crate only handles compression/decompression.
//! To decode the SSZ data into concrete beacon types, use the [Lighthouse `types`](https://github.com/sigp/lighthouse/tree/stable/consensus/types)
//! crate or another SSZ-compatible library.
//!
//! # Examples
//!
//! ## Decoding a [`CompressedBeaconState`]
//!
//! ```ignore
//! use types::{BeaconState, ChainSpec, MainnetEthSpec};
//! use reth_era::era::types::consensus::CompressedBeaconState;
//!
//! fn decode_state(
//!     compressed_state: &CompressedBeaconState,
//! ) -> Result<(), Box<dyn std::error::Error>> {
//!     let spec = ChainSpec::mainnet();
//!
//!     // Decompress to get SSZ bytes
//!     let ssz_bytes = compressed_state.decompress()?;
//!
//!     // Decode with fork-aware method, chainSpec determines fork from slot in SSZ
//!     let state = BeaconState::<MainnetEthSpec>::from_ssz_bytes(&ssz_bytes, &spec)
//!         .map_err(|e| format!("{:?}", e))?;
//!
//!     println!("State slot: {}", state.slot());
//!     println!("Fork: {:?}", state.fork_name_unchecked());
//!     println!("Validators: {}", state.validators().len());
//!     println!("Finalized checkpoint: {:?}", state.finalized_checkpoint());
//!     Ok(())
//! }
//! ```
//!
//! ## Decoding a [`CompressedSignedBeaconBlock`]
//!
//! ```ignore
//! use consensus_types::{ForkName, ForkVersionDecode, MainnetEthSpec, SignedBeaconBlock};
//! use reth_era::era::types::consensus::CompressedSignedBeaconBlock;
//!
//! // Decode using fork-aware decoding, fork must be known beforehand
//! fn decode_block(
//!     compressed: &CompressedSignedBeaconBlock,
//!     fork: ForkName,
//! ) -> Result<(), Box<dyn std::error::Error>> {
//!     // Decompress to get SSZ bytes
//!     let ssz_bytes = compressed.decompress()?;
//!
//!     let block = SignedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_by_fork(&ssz_bytes, fork)
//!         .map_err(|e| format!("{:?}", e))?;
//!
//!     println!("Block slot: {}", block.message().slot());
//!     println!("Proposer index: {}", block.message().proposer_index());
//!     println!("Parent root: {:?}", block.message().parent_root());
//!     println!("State root: {:?}", block.message().state_root());
//!
//!     Ok(())
//! }
//! ```
use crate::e2s::{error::E2sError, types::Entry};
use snap::{read::FrameDecoder, write::FrameEncoder};
use std::io::{Read, Write};

/// Maximum allowed decompressed size for a signed beacon block SSZ payload.
const MAX_DECOMPRESSED_SIGNED_BEACON_BLOCK_BYTES: usize = 256 * 1024 * 1024; // 256 MiB

/// Maximum allowed decompressed size for a beacon state SSZ payload.
const MAX_DECOMPRESSED_BEACON_STATE_BYTES: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

fn decompress_snappy_bounded(
    compressed: &[u8],
    max_decompressed_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, E2sError> {
    let mut decoder = FrameDecoder::new(compressed).take(max_decompressed_bytes as u64);
    let mut decompressed = Vec::new();

    Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
        E2sError::SnappyDecompression(format!("Failed to decompress {what}: {e}"))
    })?;

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
        decompress_snappy_bounded(
            self.data.as_slice(),
            MAX_DECOMPRESSED_SIGNED_BEACON_BLOCK_BYTES,
            "signed beacon block",
        )
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
        decompress_snappy_bounded(
            self.data.as_slice(),
            MAX_DECOMPRESSED_BEACON_STATE_BYTES,
            "beacon state",
        )
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
        let recovered_ssz = recovered.decompress().unwrap();

        assert_eq!(recovered_ssz, ssz_data);
    }

    #[test]
    fn test_entry_conversion_beacon_state() {
        let ssz_data = vec![5, 4, 3, 2, 1];
        let compressed_state = CompressedBeaconState::from_ssz(&ssz_data).unwrap();

        let entry = compressed_state.to_entry();
        assert_eq!(entry.entry_type, COMPRESSED_BEACON_STATE);

        let recovered = CompressedBeaconState::from_entry(&entry).unwrap();
        let recovered_ssz = recovered.decompress().unwrap();

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

    #[test]
    fn test_bounded_decompression_rejects_oversized_output() {
        let ssz_data = vec![42u8; 1024];
        let compressed = CompressedBeaconState::from_ssz(&ssz_data).unwrap();

        let err = decompress_snappy_bounded(compressed.data.as_slice(), 100, "beacon state")
            .unwrap_err();

        assert!(format!("{err:?}").contains("exceeded limit"));
    }
}
