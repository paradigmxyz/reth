//! Execution layer specific types for era1 files
//!
//! Contains implementations for compressed execution layer data structures:
//! - [`CompressedHeader`] - Block header
//! - [`CompressedBody`] - Block body
//! - [`CompressedReceipts`] - Block receipts
//! - [`TotalDifficulty`] - Block total difficulty
//!
//! These types use Snappy compression to match the specification.
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::e2s_types::{E2sError, Entry};
use alloy_primitives::{B256, U256};
use snap::raw::{Decoder, Encoder};

// Era1-specific constants
/// `CompressedHeader` record type
pub const COMPRESSED_HEADER: [u8; 2] = [0x03, 0x00];

/// `CompressedBody` record type
pub const COMPRESSED_BODY: [u8; 2] = [0x04, 0x00];

/// `CompressedReceipts` record type
pub const COMPRESSED_RECEIPTS: [u8; 2] = [0x05, 0x00];

/// `TotalDifficulty` record type
pub const TOTAL_DIFFICULTY: [u8; 2] = [0x06, 0x00];

/// `Accumulator` record type
pub const ACCUMULATOR: [u8; 2] = [0x07, 0x00];

/// Maximum number of blocks in an Era1 file, limited by accumulator size
pub const MAX_BLOCKS_PER_ERA1: usize = 8192;

/// Compressed block header using `snappyFramed(rlp(header))`
#[derive(Debug, Clone)]
pub struct CompressedHeader {
    /// The compressed data
    pub data: Vec<u8>,
}

impl CompressedHeader {
    /// Create a new [`CompressedHeader`] from compressed data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded header by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut encoder = Encoder::new();
        let compressed = encoder.compress_vec(rlp_data).map_err(|e| {
            E2sError::SnappyCompression(format!("Failed to compress header: {}", e))
        })?;

        Ok(Self { data: compressed })
    }

    /// Decompress to get the original RLP-encoded header
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(&self.data).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress header: {}", e))
        })?;

        Ok(decompressed)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_HEADER, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != COMPRESSED_HEADER {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for CompressedHeader: expected {:02x}{:02x}, got {:02x}{:02x}",
                COMPRESSED_HEADER[0],
                COMPRESSED_HEADER[1],
                entry.entry_type[0],
                entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }
}

/// Compressed block body using `snappyFramed(rlp(body))`
#[derive(Debug, Clone)]
pub struct CompressedBody {
    /// The compressed data
    pub data: Vec<u8>,
}

impl CompressedBody {
    /// Create a new [`CompressedBody`] from compressed data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded body by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut encoder = Encoder::new();
        let compressed = encoder
            .compress_vec(rlp_data)
            .map_err(|e| E2sError::SnappyCompression(format!("Failed to compress body: {}", e)))?;

        Ok(Self { data: compressed })
    }

    /// Decompress to get the original RLP-encoded body
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(&self.data).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress body: {}", e))
        })?;

        Ok(decompressed)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_BODY, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != COMPRESSED_BODY {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for CompressedBody: expected {:02x}{:02x}, got {:02x}{:02x}",
                COMPRESSED_BODY[0], COMPRESSED_BODY[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }
}

/// Compressed receipts using snappyFramed(rlp(receipts))
#[derive(Debug, Clone)]
pub struct CompressedReceipts {
    /// The compressed data
    pub data: Vec<u8>,
}

impl CompressedReceipts {
    /// Create a new [`CompressedReceipts`] from compressed data
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded receipts by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut encoder = Encoder::new();
        let compressed = encoder.compress_vec(rlp_data).map_err(|e| {
            E2sError::SnappyCompression(format!("Failed to compress receipts: {}", e))
        })?;

        Ok(Self { data: compressed })
    }

    /// Decompress to get the original RLP-encoded receipts
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = Decoder::new();
        let decompressed = decoder.decompress_vec(&self.data).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress receipts: {}", e))
        })?;

        Ok(decompressed)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_RECEIPTS, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != COMPRESSED_RECEIPTS {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for CompressedReceipts: expected {:02x}{:02x}, got {:02x}{:02x}",
                COMPRESSED_RECEIPTS[0], COMPRESSED_RECEIPTS[1],
                entry.entry_type[0], entry.entry_type[1]
            )));
        }

        Ok(Self { data: entry.data.clone() })
    }
}

/// Total difficulty for a block
#[derive(Debug, Clone)]
pub struct TotalDifficulty {
    /// The total difficulty as U256
    pub value: U256,
}

impl TotalDifficulty {
    /// Create a new [`TotalDifficulty`] from a U256 value
    pub fn new(value: U256) -> Self {
        Self { value }
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        let mut data = [0u8; 32];

        let be_bytes = self.value.to_be_bytes_vec();

        if be_bytes.len() <= 32 {
            data[32 - be_bytes.len()..].copy_from_slice(&be_bytes);
        } else {
            data.copy_from_slice(&be_bytes[be_bytes.len() - 32..]);
        }

        Entry::new(TOTAL_DIFFICULTY, data.to_vec())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != TOTAL_DIFFICULTY {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for TotalDifficulty: expected {:02x}{:02x}, got {:02x}{:02x}",
                TOTAL_DIFFICULTY[0], TOTAL_DIFFICULTY[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        if entry.data.len() != 32 {
            return Err(E2sError::Ssz(format!(
                "Invalid data length for TotalDifficulty: expected 32, got {}",
                entry.data.len()
            )));
        }

        // Convert 32-byte array to U256
        let value = U256::from_be_slice(&entry.data);

        Ok(Self { value })
    }
}

/// Accumulator is computed by constructing an SSZ list of header-records
/// and calculating the `hash_tree_root`
#[derive(Debug, Clone)]
pub struct Accumulator {
    /// The accumulator root hash
    pub root: B256,
}

impl Accumulator {
    /// Create a new [`Accumulator`] from a root hash
    pub fn new(root: B256) -> Self {
        Self { root }
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(ACCUMULATOR, self.root.to_vec())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != ACCUMULATOR {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for Accumulator: expected {:02x}{:02x}, got {:02x}{:02x}",
                ACCUMULATOR[0], ACCUMULATOR[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        if entry.data.len() != 32 {
            return Err(E2sError::Ssz(format!(
                "Invalid data length for Accumulator: expected 32, got {}",
                entry.data.len()
            )));
        }

        let mut root = [0u8; 32];
        root.copy_from_slice(&entry.data);

        Ok(Self { root: B256::from(root) })
    }
}

/// A block tuple in an Era1 file, containing all components for a single block
#[derive(Debug, Clone)]
pub struct BlockTuple {
    /// Compressed block header
    pub header: CompressedHeader,

    /// Compressed block body
    pub body: CompressedBody,

    /// Compressed receipts
    pub receipts: CompressedReceipts,

    /// Total difficulty
    pub total_difficulty: TotalDifficulty,
}

impl BlockTuple {
    /// Create a new [`BlockTuple`]
    pub fn new(
        header: CompressedHeader,
        body: CompressedBody,
        receipts: CompressedReceipts,
        total_difficulty: TotalDifficulty,
    ) -> Self {
        Self { header, body, receipts, total_difficulty }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_difficulty_roundtrip() {
        let value = U256::from(123456789u64);

        let total_difficulty = TotalDifficulty::new(value);

        let entry = total_difficulty.to_entry();

        // Validate entry type
        assert_eq!(entry.entry_type, TOTAL_DIFFICULTY);

        let recovered = TotalDifficulty::from_entry(&entry).unwrap();

        // Verify values match
        assert_eq!(recovered.value, value);
    }

    #[test]
    fn test_compression_roundtrip() {

        let rlp_data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        // Test header compression/decompression
        let compressed_header = CompressedHeader::from_rlp(&rlp_data).unwrap();
        let decompressed = compressed_header.decompress().unwrap();
        assert_eq!(decompressed, rlp_data);

        // Test body compression/decompression
        let compressed_body = CompressedBody::from_rlp(&rlp_data).unwrap();
        let decompressed = compressed_body.decompress().unwrap();
        assert_eq!(decompressed, rlp_data);

        // Test receipts compression/decompression
        let compressed_receipts = CompressedReceipts::from_rlp(&rlp_data).unwrap();
        let decompressed = compressed_receipts.decompress().unwrap();
        assert_eq!(decompressed, rlp_data);
    }
}
