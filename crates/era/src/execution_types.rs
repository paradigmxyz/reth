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
use alloy_consensus::{Block, BlockBody, Header};
use alloy_primitives::{B256, U256};
use alloy_rlp::{Decodable, Encodable};
use snap::{read::FrameDecoder, write::FrameEncoder};
use std::{
    io::{Read, Write},
    marker::PhantomData,
};

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

/// Generic codec for Snappy-compressed RLP data
#[derive(Debug, Clone, Default)]
pub struct SnappyRlpCodec<T> {
    _phantom: PhantomData<T>,
}

impl<T> SnappyRlpCodec<T> {
    /// Create a new codec for the given type
    pub const fn new() -> Self {
        Self { _phantom: PhantomData }
    }
}

impl<T: Decodable> SnappyRlpCodec<T> {
    /// Decode compressed data into the target type
    pub fn decode(&self, compressed_data: &[u8]) -> Result<T, E2sError> {
        let mut decoder = FrameDecoder::new(compressed_data);
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress data: {e}"))
        })?;

        let mut slice = decompressed.as_slice();
        T::decode(&mut slice).map_err(|e| E2sError::Rlp(format!("Failed to decode RLP data: {e}")))
    }
}

impl<T: Encodable> SnappyRlpCodec<T> {
    /// Encode data into compressed format
    pub fn encode(&self, data: &T) -> Result<Vec<u8>, E2sError> {
        let mut rlp_data = Vec::new();
        data.encode(&mut rlp_data);

        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, &rlp_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress data: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }

        Ok(compressed)
    }
}

/// Compressed block header using `snappyFramed(rlp(header))`
#[derive(Debug, Clone)]
pub struct CompressedHeader {
    /// The compressed data
    pub data: Vec<u8>,
}

/// Extension trait for generic decoding from compressed data
pub trait DecodeCompressed {
    /// Decompress and decode the data into the given type
    fn decode<T: Decodable>(&self) -> Result<T, E2sError>;
}

impl CompressedHeader {
    /// Create a new [`CompressedHeader`] from compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded header by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, rlp_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress header: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }
        Ok(Self { data: compressed })
    }

    /// Decompress to get the original RLP-encoded header
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = FrameDecoder::new(self.data.as_slice());
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress header: {e}"))
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

    /// Decode this compressed header into an `alloy_consensus::Header`
    pub fn decode_header(&self) -> Result<Header, E2sError> {
        self.decode()
    }

    /// Create a [`CompressedHeader`] from a header.
    pub fn from_header<H: Encodable>(header: &H) -> Result<Self, E2sError> {
        let encoder = SnappyRlpCodec::new();
        let compressed = encoder.encode(header)?;
        Ok(Self::new(compressed))
    }
}

impl DecodeCompressed for CompressedHeader {
    fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
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
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded body by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, rlp_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress header: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }
        Ok(Self { data: compressed })
    }

    /// Decompress to get the original RLP-encoded body
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = FrameDecoder::new(self.data.as_slice());
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress body: {e}"))
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

    /// Decode this [`CompressedBody`] into an `alloy_consensus::BlockBody`
    pub fn decode_body<T: Decodable, H: Decodable>(&self) -> Result<BlockBody<T, H>, E2sError> {
        let decompressed = self.decompress()?;
        Self::decode_body_from_decompressed(&decompressed)
    }

    /// Decode decompressed body data into an `alloy_consensus::BlockBody`
    pub fn decode_body_from_decompressed<T: Decodable, H: Decodable>(
        data: &[u8],
    ) -> Result<BlockBody<T, H>, E2sError> {
        alloy_rlp::decode_exact::<BlockBody<T, H>>(data)
            .map_err(|e| E2sError::Rlp(format!("Failed to decode RLP data: {e}")))
    }

    /// Create a [`CompressedBody`] from a block body (e.g.  `alloy_consensus::BlockBody`)
    pub fn from_body<B: Encodable>(body: &B) -> Result<Self, E2sError> {
        let encoder = SnappyRlpCodec::new();
        let compressed = encoder.encode(body)?;
        Ok(Self::new(compressed))
    }
}

impl DecodeCompressed for CompressedBody {
    fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
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
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded receipts by compressing it with Snappy
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        let mut compressed = Vec::new();
        {
            let mut encoder = FrameEncoder::new(&mut compressed);

            Write::write_all(&mut encoder, rlp_data).map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to compress header: {e}"))
            })?;

            encoder.flush().map_err(|e| {
                E2sError::SnappyCompression(format!("Failed to flush encoder: {e}"))
            })?;
        }
        Ok(Self { data: compressed })
    }
    /// Decompress to get the original RLP-encoded receipts
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        let mut decoder = FrameDecoder::new(self.data.as_slice());
        let mut decompressed = Vec::new();
        Read::read_to_end(&mut decoder, &mut decompressed).map_err(|e| {
            E2sError::SnappyDecompression(format!("Failed to decompress receipts: {e}"))
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

    /// Decode this [`CompressedReceipts`] into the given type
    pub fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
    }

    /// Create [`CompressedReceipts`] from an encodable type
    pub fn from_encodable<T: Encodable>(data: &T) -> Result<Self, E2sError> {
        let encoder = SnappyRlpCodec::<T>::new();
        let compressed = encoder.encode(data)?;
        Ok(Self::new(compressed))
    }
    /// Encode a list of receipts to RLP format
    pub fn encode_receipts_to_rlp<T: Encodable>(receipts: &[T]) -> Result<Vec<u8>, E2sError> {
        let mut rlp_data = Vec::new();
        alloy_rlp::encode_list(receipts, &mut rlp_data);
        Ok(rlp_data)
    }

    /// Encode and compress a list of receipts
    pub fn from_encodable_list<T: Encodable>(receipts: &[T]) -> Result<Self, E2sError> {
        let rlp_data = Self::encode_receipts_to_rlp(receipts)?;
        Self::from_rlp(&rlp_data)
    }
}

impl DecodeCompressed for CompressedReceipts {
    fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
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
    pub const fn new(value: U256) -> Self {
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
    pub const fn new(root: B256) -> Self {
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
    pub const fn new(
        header: CompressedHeader,
        body: CompressedBody,
        receipts: CompressedReceipts,
        total_difficulty: TotalDifficulty,
    ) -> Self {
        Self { header, body, receipts, total_difficulty }
    }

    /// Convert to an `alloy_consensus::Block`
    pub fn to_alloy_block<T: Decodable>(&self) -> Result<Block<T>, E2sError> {
        let header: Header = self.header.decode()?;
        let body: BlockBody<T> = self.body.decode()?;

        Ok(Block::new(header, body))
    }

    /// Create from an `alloy_consensus::Block`
    pub fn from_alloy_block<T: Encodable, R: Encodable>(
        block: &Block<T>,
        receipts: &R,
        total_difficulty: U256,
    ) -> Result<Self, E2sError> {
        let header = CompressedHeader::from_header(&block.header)?;
        let body = CompressedBody::from_body(&block.body)?;

        let compressed_receipts = CompressedReceipts::from_encodable(receipts)?;

        let difficulty = TotalDifficulty::new(total_difficulty);

        Ok(Self::new(header, body, compressed_receipts, difficulty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip4895::Withdrawals;
    use alloy_primitives::{Address, Bytes, B64};

    #[test]
    fn test_header_conversion_roundtrip() {
        let header = Header {
            parent_hash: B256::default(),
            ommers_hash: B256::default(),
            beneficiary: Address::default(),
            state_root: B256::default(),
            transactions_root: B256::default(),
            receipts_root: B256::default(),
            logs_bloom: Default::default(),
            difficulty: U256::from(123456u64),
            number: 100,
            gas_limit: 5000000,
            gas_used: 21000,
            timestamp: 1609459200,
            extra_data: Bytes::default(),
            mix_hash: B256::default(),
            nonce: B64::default(),
            base_fee_per_gas: Some(10),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
        };

        let compressed_header = CompressedHeader::from_header(&header).unwrap();

        let decoded_header = compressed_header.decode_header().unwrap();

        assert_eq!(header.number, decoded_header.number);
        assert_eq!(header.difficulty, decoded_header.difficulty);
        assert_eq!(header.timestamp, decoded_header.timestamp);
        assert_eq!(header.gas_used, decoded_header.gas_used);
        assert_eq!(header.parent_hash, decoded_header.parent_hash);
        assert_eq!(header.base_fee_per_gas, decoded_header.base_fee_per_gas);
    }

    #[test]
    fn test_block_body_conversion() {
        let block_body: BlockBody<Bytes> =
            BlockBody { transactions: vec![], ommers: vec![], withdrawals: None };

        let compressed_body = CompressedBody::from_body(&block_body).unwrap();

        let decoded_body: BlockBody<Bytes> = compressed_body.decode_body().unwrap();

        assert_eq!(decoded_body.transactions.len(), 0);
        assert_eq!(decoded_body.ommers.len(), 0);
        assert_eq!(decoded_body.withdrawals, None);
    }

    #[test]
    fn test_total_difficulty_roundtrip() {
        let value = U256::from(123456789u64);

        let total_difficulty = TotalDifficulty::new(value);

        let entry = total_difficulty.to_entry();

        assert_eq!(entry.entry_type, TOTAL_DIFFICULTY);

        let recovered = TotalDifficulty::from_entry(&entry).unwrap();

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

    #[test]
    fn test_block_tuple_with_data() {
        // Create block with transactions and withdrawals
        let header = Header {
            parent_hash: B256::default(),
            ommers_hash: B256::default(),
            beneficiary: Address::default(),
            state_root: B256::default(),
            transactions_root: B256::default(),
            receipts_root: B256::default(),
            logs_bloom: Default::default(),
            difficulty: U256::from(123456u64),
            number: 100,
            gas_limit: 5000000,
            gas_used: 21000,
            timestamp: 1609459200,
            extra_data: Bytes::default(),
            mix_hash: B256::default(),
            nonce: B64::default(),
            base_fee_per_gas: Some(10),
            withdrawals_root: Some(B256::default()),
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
        };

        let transactions = vec![Bytes::from(vec![1, 2, 3, 4]), Bytes::from(vec![5, 6, 7, 8])];

        let withdrawals = Some(Withdrawals(vec![]));

        let block_body = BlockBody { transactions, ommers: vec![], withdrawals };

        let block = Block::new(header, block_body);

        let receipts: Vec<u8> = Vec::new();

        let block_tuple =
            BlockTuple::from_alloy_block(&block, &receipts, U256::from(123456u64)).unwrap();

        // Convert back to Block
        let decoded_block: Block<Bytes> = block_tuple.to_alloy_block().unwrap();

        // Verify block components
        assert_eq!(decoded_block.header.number, 100);
        assert_eq!(decoded_block.body.transactions.len(), 2);
        assert_eq!(decoded_block.body.transactions[0], Bytes::from(vec![1, 2, 3, 4]));
        assert_eq!(decoded_block.body.transactions[1], Bytes::from(vec![5, 6, 7, 8]));
        assert!(decoded_block.body.withdrawals.is_some());
    }
}
