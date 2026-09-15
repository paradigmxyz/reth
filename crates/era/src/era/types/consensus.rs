//! Consensus types for Era post-merge history files
//!
//! # Decoding
//!
//! This crate handles compression/decompression and, for post-merge blocks, extraction of the
//! embedded execution block via [`CompressedSignedBeaconBlock::decode_execution_block`]. The
//! decompressed bytes are SSZ-encoded consensus types decoded with [`alloy_rpc_types_beacon`]'s
//! fork-specific beacon block types — no external consensus client is required.
//!
//! [`alloy_rpc_types_beacon`]: https://docs.rs/alloy-rpc-types-beacon
//!
//! # Example
//!
//! ## Decoding the execution block from a [`CompressedSignedBeaconBlock`]
//!
//! ```no_run
//! use reth_era::era::types::consensus::CompressedSignedBeaconBlock;
//! use reth_ethereum_primitives::TransactionSigned;
//!
//! fn decode_block(
//!     compressed: &CompressedSignedBeaconBlock,
//! ) -> Result<(), Box<dyn std::error::Error>> {
//!     // Post-merge blocks carry an execution payload; pre-merge slots yield `None`.
//!     if let Some(block) = compressed.decode_execution_block::<TransactionSigned>()? {
//!         println!("Execution block number: {}", block.header.number);
//!     }
//!     Ok(())
//! }
//! ```
use crate::e2s::{error::E2sError, types::Entry};
use alloy_consensus::Block;
use alloy_eips::eip2718::Decodable2718;
use alloy_rpc_types_beacon::block::{
    SignedBeaconBlockAltair, SignedBeaconBlockBellatrix, SignedBeaconBlockCapella,
    SignedBeaconBlockDeneb, SignedBeaconBlockElectra, SignedBeaconBlockPhase0,
};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1,
    ExecutionPayloadV2, ExecutionPayloadV3, PraguePayloadFields,
};
use snap::{read::FrameDecoder, write::FrameEncoder};
use ssz::Decode;
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

    Read::read_to_end(&mut decoder, &mut decompressed)
        .map_err(|e| E2sError::SnappyDecompression(format!("Failed to decompress {what}: {e}")))?;

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

    /// Decodes the execution block embedded in this beacon block, if it has one.
    ///
    /// Only post-merge (Bellatrix and later) blocks carry a payload; the fork is found by
    /// trial-decoding newest to oldest. Returns `Ok(None)` for genuine pre-merge slots, confirmed
    /// to decode as a pre-merge block. Bytes matching no known fork are an error, so malformed data
    /// is never silently dropped.
    pub fn decode_execution_block<T: Decodable2718>(&self) -> Result<Option<Block<T>>, E2sError> {
        let ssz = self.decompress()?;

        if let Ok(beacon) = SignedBeaconBlockElectra::<ExecutionPayloadV3>::from_ssz_bytes(&ssz) {
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root: beacon.message.parent_root,
                    versioned_hashes: Vec::new(),
                },
                PraguePayloadFields::new(beacon.message.body.execution_requests.to_requests()),
            );
            let payload = ExecutionPayload::V3(beacon.message.body.execution_payload);
            return Ok(Some(payload.try_into_block_with_sidecar(&sidecar)?));
        }

        if let Ok(beacon) = SignedBeaconBlockDeneb::<ExecutionPayloadV3>::from_ssz_bytes(&ssz) {
            let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root: beacon.message.parent_root,
                versioned_hashes: Vec::new(),
            });
            let payload = ExecutionPayload::V3(beacon.message.body.execution_payload);
            return Ok(Some(payload.try_into_block_with_sidecar(&sidecar)?));
        }

        if let Ok(beacon) = SignedBeaconBlockCapella::<ExecutionPayloadV2>::from_ssz_bytes(&ssz) {
            let payload = ExecutionPayload::V2(beacon.message.body.execution_payload);
            return Ok(Some(payload.try_into_block()?));
        }

        if let Ok(beacon) = SignedBeaconBlockBellatrix::<ExecutionPayloadV1>::from_ssz_bytes(&ssz) {
            let payload = ExecutionPayload::V1(beacon.message.body.execution_payload);
            return Ok(Some(payload.try_into_block()?));
        }

        // Pre-merge blocks carry no execution payload. Only skip a slot once it's confirmed to be a
        // valid pre-merge block; anything else is malformed data and must error rather than skip.
        if SignedBeaconBlockPhase0::from_ssz_bytes(&ssz).is_ok() ||
            SignedBeaconBlockAltair::from_ssz_bytes(&ssz).is_ok()
        {
            return Ok(None);
        }

        Err(E2sError::Ssz(format!(
            "consensus block ({} bytes) is not a valid SignedBeaconBlock of any known fork",
            ssz.len()
        )))
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
    use alloy_primitives::B256;
    use alloy_rpc_types_beacon::{
        block::{BeaconBlock, BeaconBlockBodyPhase0, Eth1Data, SignedBeaconBlock},
        BlsSignature,
    };
    use reth_ethereum_primitives::TransactionSigned;
    use ssz::Encode;

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

        let err =
            decompress_snappy_bounded(compressed.data.as_slice(), 100, "beacon state").unwrap_err();

        assert!(format!("{err:?}").contains("exceeded limit"));
    }

    #[test]
    fn decode_execution_block_skips_genuine_pre_merge() {
        // A genuine pre-merge (phase0) beacon block carries no execution payload and yields `None`.
        let block = SignedBeaconBlock {
            message: BeaconBlock {
                slot: 0,
                proposer_index: 0,
                parent_root: B256::ZERO,
                state_root: B256::ZERO,
                body: BeaconBlockBodyPhase0 {
                    randao_reveal: BlsSignature::ZERO,
                    eth1_data: Eth1Data {
                        deposit_root: B256::ZERO,
                        deposit_count: 0,
                        block_hash: B256::ZERO,
                    },
                    graffiti: B256::ZERO,
                    proposer_slashings: vec![],
                    attester_slashings: vec![],
                    attestations: vec![],
                    deposits: vec![],
                    voluntary_exits: vec![],
                },
            },
            signature: BlsSignature::ZERO,
        };
        let compressed = CompressedSignedBeaconBlock::from_ssz(&block.as_ssz_bytes()).unwrap();
        assert!(compressed.decode_execution_block::<TransactionSigned>().unwrap().is_none());
    }

    #[test]
    fn decode_execution_block_errors_on_malformed() {
        // Bytes that decode as no known fork are malformed, not pre-merge slots, and must error
        // instead of being silently dropped.
        for bytes in [vec![], vec![0u8; 8]] {
            let compressed = CompressedSignedBeaconBlock::from_ssz(&bytes).unwrap();
            assert!(compressed.decode_execution_block::<TransactionSigned>().is_err());
        }
    }
}
