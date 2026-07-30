//! Execution layer specific types for `.ere` files
//!
//! Contains implementations for compressed execution layer data structures:
//! - [`CompressedHeader`] - Block header
//! - [`CompressedBody`] - Block body
//! - [`CompressedSlimReceipts`] - Block receipts
//! - [`TotalDifficulty`] - Block total difficulty
//!
//! These types use Snappy compression to match the specification.
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md>

use crate::{
    common::{
        compression::{snappy_compress, snappy_decompress, SnappyRlpCodec},
        decode::DecodeCompressedRlp,
    },
    e2s::{error::E2sError, types::Entry},
};
use alloy_consensus::{Block, BlockBody, Eip658Value, Header, TxType};
use alloy_primitives::{Log, B256, U256};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use sha2::{Digest, Sha256};

// ERE-specific constants
/// `CompressedHeader` record type
pub const COMPRESSED_HEADER: [u8; 2] = [0x03, 0x00];

/// `CompressedBody` record type
pub const COMPRESSED_BODY: [u8; 2] = [0x04, 0x00];

/// `CompressedSlimReceipts` record type (0x0a00)
/// Slim receipts exclude bloom filters to optimize storage.
pub const COMPRESSED_SLIM_RECEIPTS: [u8; 2] = [0x0a, 0x00];

/// `Proof` record type (0x0b00)
/// Format: `snappyFramed(rlp([proof-type, ssz(proof-object)]))`
pub const PROOF: [u8; 2] = [0x0b, 0x00];

/// `TotalDifficulty` record type
pub const TOTAL_DIFFICULTY: [u8; 2] = [0x06, 0x00];

/// `Accumulator` record type
pub const ACCUMULATOR: [u8; 2] = [0x07, 0x00];

/// Maximum number of blocks in an `ERE` file, limited by accumulator size.
pub const MAX_BLOCKS_PER_ERE: usize = crate::common::MAX_ENTRIES_PER_ERA as usize;

/// Compressed block header using `snappyFramed(rlp(header))`
#[derive(Debug, Clone)]
pub struct CompressedHeader {
    /// The compressed data
    pub data: Vec<u8>,
}

impl CompressedHeader {
    /// Create a new [`CompressedHeader`] from compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded header by compressing it with Snappy framed encoding
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        Ok(Self { data: snappy_compress(rlp_data)? })
    }

    /// Decompress to get the original RLP-encoded header
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        snappy_decompress(&self.data)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_HEADER, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(COMPRESSED_HEADER, "CompressedHeader")?;
        Ok(Self { data: entry.data.clone() })
    }

    /// Decode this compressed header into an `alloy_consensus::Header`
    pub fn decode_header(&self) -> Result<Header, E2sError> {
        self.decode()
    }

    /// Create a [`CompressedHeader`] from a header
    pub fn from_header<H: Encodable>(header: &H) -> Result<Self, E2sError> {
        let encoder = SnappyRlpCodec::new();
        let compressed = encoder.encode(header)?;
        Ok(Self::new(compressed))
    }
}

impl DecodeCompressedRlp for CompressedHeader {
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

    /// Create from RLP-encoded body by compressing it with Snappy framed encoding
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        Ok(Self { data: snappy_compress(rlp_data)? })
    }

    /// Decompress to get the original RLP-encoded body
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        snappy_decompress(&self.data)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_BODY, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(COMPRESSED_BODY, "CompressedBody")?;
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

impl DecodeCompressedRlp for CompressedBody {
    fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
    }
}

/// Compressed slim receipts using `snappyFramed(rlp(...))`.
///
/// Slim receipts exclude bloom filters to optimize storage.
/// Format: `snappyFramed(rlp([tx-type, post-state-or-status, cumulative-gas, logs]))`
#[derive(Debug, Clone)]
pub struct CompressedSlimReceipts {
    /// The compressed data
    pub data: Vec<u8>,
}

impl CompressedSlimReceipts {
    /// Create a new [`CompressedSlimReceipts`] from compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create from RLP-encoded slim receipts by compressing with Snappy framed encoding
    pub fn from_rlp(rlp_data: &[u8]) -> Result<Self, E2sError> {
        Ok(Self { data: snappy_compress(rlp_data)? })
    }

    /// Decompress to get the original RLP-encoded slim receipts
    pub fn decompress(&self) -> Result<Vec<u8>, E2sError> {
        snappy_decompress(&self.data)
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(COMPRESSED_SLIM_RECEIPTS, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(COMPRESSED_SLIM_RECEIPTS, "CompressedSlimReceipts")?;
        Ok(Self { data: entry.data.clone() })
    }

    /// Decode this [`CompressedSlimReceipts`] into the given type
    pub fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
    }

    /// Create [`CompressedSlimReceipts`] from an encodable type
    pub fn from_encodable<T: Encodable>(data: &T) -> Result<Self, E2sError> {
        let encoder = SnappyRlpCodec::<T>::new();
        let compressed = encoder.encode(data)?;
        Ok(Self::new(compressed))
    }

    /// Encode and compress a list of slim receipts
    pub fn from_encodable_list<T: Encodable>(receipts: &[T]) -> Result<Self, E2sError> {
        let mut rlp_data = Vec::new();
        alloy_rlp::encode_list(receipts, &mut rlp_data);
        Self::from_rlp(&rlp_data)
    }

    /// Compress a block's slim receipts.
    ///
    /// [`SlimReceipt`] is the canonical slim receipt: its RLP encoding is the 4-element list
    /// `[tx-type, post-state-or-status, cumulative-gas, logs]`, with no bloom filter.
    pub fn from_receipts(receipts: &[SlimReceipt]) -> Result<Self, E2sError> {
        Self::from_encodable_list(receipts)
    }

    /// Decompress and decode this entry into a block's slim receipts.
    pub fn decode_receipts(&self) -> Result<Vec<SlimReceipt>, E2sError> {
        self.decode()
    }
}

impl DecodeCompressedRlp for CompressedSlimReceipts {
    fn decode<T: Decodable>(&self) -> Result<T, E2sError> {
        let decoder = SnappyRlpCodec::<T>::new();
        decoder.decode(&self.data)
    }
}

/// A slim execution receipt as stored in an `ERE` file.
///
/// Per the spec, the slim form is the 4-element RLP list
/// `[tx-type, post-state-or-status, cumulative-gas, logs]` with **no bloom filter** (the bloom is
/// recomputable from the logs). This is a thin wrapper over alloy's field types: [`Eip658Value`]
/// captures both the pre-Byzantium 32-byte post-state root and the post-Byzantium boolean status,
/// so a single type decodes receipts across every fork.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct SlimReceipt {
    /// Transaction type (EIP-2718).
    pub tx_type: TxType,
    /// Post-state root (pre-Byzantium) or success status (post-Byzantium).
    pub status: Eip658Value,
    /// Cumulative gas used in the block up to and including this transaction.
    pub cumulative_gas_used: u64,
    /// Logs emitted by the transaction.
    pub logs: Vec<Log>,
}

/// Proof type discriminant used inside the Proof entry's RLP envelope.
///
/// Maps to specific Portal Network proof objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProofType {
    /// Pre-merge proof against the historical hashes accumulator
    BlockProofHistoricalHashesAccumulator = 0,
    /// Post-merge proof against historical roots
    BlockProofHistoricalRoots = 1,
    /// Capella-era proof against historical summaries
    BlockProofHistoricalSummariesCapella = 2,
    /// Deneb-era proof against historical summaries
    BlockProofHistoricalSummariesDeneb = 3,
}

impl ProofType {
    /// Convert from a raw byte value
    pub const fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::BlockProofHistoricalHashesAccumulator),
            1 => Some(Self::BlockProofHistoricalRoots),
            2 => Some(Self::BlockProofHistoricalSummariesCapella),
            3 => Some(Self::BlockProofHistoricalSummariesDeneb),
            _ => None,
        }
    }

    /// Convert to a raw byte value
    pub const fn as_byte(self) -> u8 {
        self as u8
    }
}

/// A proof entry attesting to block validity against a trusted consensus layer header.
///
/// Format: `snappyFramed(rlp([proof-type, ssz(proof-object)]))`
///
/// Multiple proof types can coexist in the same file at fork boundaries.
#[derive(Debug, Clone)]
pub struct Proof {
    /// The compressed data containing `rlp([proof-type, ssz(proof-object)])`
    pub data: Vec<u8>,
}

impl Proof {
    /// Create a new [`Proof`] from already-compressed data
    pub const fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Encode a [`Proof`] from a proof type and raw SSZ-encoded proof object.
    pub fn encode(proof_type: ProofType, ssz_proof: &[u8]) -> Result<Self, E2sError> {
        // Build the list payload first so the RLP list header gets the exact length,
        // regardless of how each item encodes.
        let mut payload = Vec::new();
        proof_type.as_byte().encode(&mut payload);
        ssz_proof.encode(&mut payload);

        let mut rlp_data = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut rlp_data);
        rlp_data.extend_from_slice(&payload);

        Ok(Self { data: snappy_compress(&rlp_data)? })
    }

    /// Decode the proof, returning `(proof_type, raw_ssz_proof_bytes)`.
    pub fn decode(&self) -> Result<(ProofType, Vec<u8>), E2sError> {
        let decompressed = snappy_decompress(&self.data)?;

        let mut buf = decompressed.as_slice();
        let header = alloy_rlp::Header::decode(&mut buf)
            .map_err(|e| E2sError::Rlp(format!("Failed to decode proof RLP header: {e}")))?;
        if !header.list {
            return Err(E2sError::Rlp("Expected RLP list for Proof entry".to_string()));
        }

        // A proof is exactly the two-item list `[proof-type, ssz(proof-object)]`. Pin decoding to
        // the list's own payload so nothing outside it slips through: bytes after the list end, or
        // a third item inside it, both fail instead of being silently dropped.
        if buf.len() != header.payload_length {
            return Err(E2sError::Rlp(format!(
                "Trailing bytes after Proof list: {} byte(s) beyond the list payload",
                buf.len().saturating_sub(header.payload_length)
            )));
        }
        let mut payload = &buf[..header.payload_length];

        let proof_type_byte = u8::decode(&mut payload)
            .map_err(|e| E2sError::Rlp(format!("Failed to decode proof type: {e}")))?;
        let proof_type = ProofType::from_byte(proof_type_byte)
            .ok_or_else(|| E2sError::Rlp(format!("Unknown proof type: {proof_type_byte}")))?;

        let ssz_bytes = alloy_primitives::Bytes::decode(&mut payload)
            .map_err(|e| E2sError::Rlp(format!("Failed to decode proof SSZ bytes: {e}")))?;

        if !payload.is_empty() {
            return Err(E2sError::Rlp("Unexpected extra items in Proof list".to_string()));
        }

        Ok((proof_type, ssz_bytes.to_vec()))
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        Entry::new(PROOF, self.data.clone())
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(PROOF, "Proof")?;
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
    pub const fn new(value: U256) -> Self {
        Self { value }
    }

    /// Convert to an [`Entry`]
    pub fn to_entry(&self) -> Entry {
        // ere spec: `total-difficulty = { type: 0x0600, data: SSZ uint256 }` (little-endian)
        let data = self.value.to_le_bytes::<32>().to_vec();
        Entry::new(TOTAL_DIFFICULTY, data)
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(TOTAL_DIFFICULTY, "TotalDifficulty")?;

        if entry.data.len() != 32 {
            return Err(E2sError::Ssz(format!(
                "Invalid data length for TotalDifficulty: expected 32, got {}",
                entry.data.len()
            )));
        }

        // ere spec: `total-difficulty = { type: 0x0600, data: SSZ uint256 }` (little-endian)
        let value = U256::from_le_slice(&entry.data);

        Ok(Self { value })
    }
}

/// Accumulator is computed by constructing an SSZ list of header-records
/// and calculating the `hash_tree_root`
#[derive(Debug, Clone, PartialEq, Eq)]
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
        entry.ensure_type(ACCUMULATOR, "Accumulator")?;

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

    /// Compute the accumulator from a list of header records.
    ///
    /// Implements `hash_tree_root(List[HeaderRecord, 8192])` per the spec:
    /// - Each leaf is `sha256(block_hash || total_difficulty_le_bytes32)`
    /// - Leaves are padded to `MAX_BLOCKS_PER_ERE` (8192) with zero hashes
    /// - Binary Merkle tree is computed bottom-up
    /// - Final root is `sha256(merkle_root || le_bytes32(actual_count))`
    ///
    /// Returns `Err` if `records` exceeds [`MAX_BLOCKS_PER_ERE`].
    pub fn from_header_records(records: &[HeaderRecord]) -> Result<Self, E2sError> {
        let capacity = MAX_BLOCKS_PER_ERE;

        if records.len() > capacity {
            return Err(E2sError::Ssz(format!(
                "Too many header records: got {}, max {}",
                records.len(),
                capacity
            )));
        }

        // Compute leaf hash for each header record
        let mut leaves = Vec::with_capacity(capacity);
        for record in records {
            let mut data = [0u8; 64];
            data[..32].copy_from_slice(record.block_hash.as_slice());
            data[32..].copy_from_slice(&record.total_difficulty.to_le_bytes::<32>());
            leaves.push(<[u8; 32]>::from(Sha256::digest(data)));
        }

        // Pad to capacity with zero hashes
        leaves.resize(capacity, [0u8; 32]);

        // Binary Merkle tree bottom-up (capacity is always a power of two)
        while leaves.len() > 1 {
            let mut next_level = Vec::with_capacity(leaves.len() / 2);
            for pair in leaves.as_chunks::<2>().0 {
                let mut data = [0u8; 64];
                data[..32].copy_from_slice(&pair[0]);
                data[32..].copy_from_slice(&pair[1]);
                next_level.push(<[u8; 32]>::from(Sha256::digest(data)));
            }
            leaves = next_level;
        }

        let merkle_root = leaves[0];

        // mix_in_length: sha256(merkle_root || le_bytes32(actual_length))
        let mut mix = [0u8; 64];
        mix[..32].copy_from_slice(&merkle_root);
        let length = records.len() as u64;
        mix[32..40].copy_from_slice(&length.to_le_bytes());
        // remaining bytes stay zero (uint256 LE padding)

        Ok(Self { root: B256::from(<[u8; 32]>::from(Sha256::digest(mix))) })
    }
}

/// The minimal per-block commitment used to build the accumulator.
///
/// This is **not** a block header: it is the 64-byte leaf
/// `{ block-hash: Bytes32, total-difficulty: Uint256 }` that
/// [`Accumulator::from_header_records`] merkleizes as `hash_tree_root(List[HeaderRecord, 8192])`.
/// The full header is stored separately as [`CompressedHeader`].
///
/// Only meaningful pre-merge, since `total-difficulty` stops advancing after the merge.
#[derive(Debug, Clone)]
pub struct HeaderRecord {
    /// The canonical block hash, i.e. `keccak256(rlp(header))` — the hash *of* the full block
    /// header, which serves as this leaf's identity in the accumulator.
    pub block_hash: B256,
    /// The **cumulative** total difficulty through this block (the running sum of every block's
    /// difficulty up to and including it), not the header's own per-block `difficulty` field.
    pub total_difficulty: U256,
}

/// A single block's components in an `ERE` file.
///
/// Only the header and body are mandatory; receipts, total difficulty, and the proof are optional,
/// so subset profiles or post-merge blocks can omit them.
/// [`component_count`](Self::component_count) reports how many are present, matching the file's
/// `DynamicBlockIndex` `component-count`.
///
/// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md#specification>
#[derive(Debug, Clone)]
pub struct BlockTuple {
    /// Compressed block header
    pub header: CompressedHeader,

    /// Compressed block body
    pub body: CompressedBody,

    /// Compressed slim receipts, omitted by the `noreceipts` profile
    pub receipts: Option<CompressedSlimReceipts>,

    /// Total difficulty, absent once it stops advancing after the merge
    pub total_difficulty: Option<TotalDifficulty>,

    /// Proof of block validity, omitted by the `noproofs` profile
    pub proof: Option<Proof>,
}

impl BlockTuple {
    /// Create a new [`BlockTuple`] with only the mandatory header and body.
    ///
    /// Attach the optional components with [`with_receipts`](Self::with_receipts),
    /// [`with_total_difficulty`](Self::with_total_difficulty), and
    /// [`with_proof`](Self::with_proof).
    pub const fn new(header: CompressedHeader, body: CompressedBody) -> Self {
        Self { header, body, receipts: None, total_difficulty: None, proof: None }
    }

    /// Attach compressed slim receipts.
    pub fn with_receipts(mut self, receipts: CompressedSlimReceipts) -> Self {
        self.receipts = Some(receipts);
        self
    }

    /// Attach the total difficulty.
    pub const fn with_total_difficulty(mut self, total_difficulty: TotalDifficulty) -> Self {
        self.total_difficulty = Some(total_difficulty);
        self
    }

    /// Attach a validity proof.
    pub fn with_proof(mut self, proof: Proof) -> Self {
        self.proof = Some(proof);
        self
    }

    /// Number of index components this block contributes: the mandatory header and body plus each
    /// optional component present. Always in the range 2-5, matching the file's `component-count`.
    pub const fn component_count(&self) -> u64 {
        2 + self.receipts.is_some() as u64 +
            self.total_difficulty.is_some() as u64 +
            self.proof.is_some() as u64
    }

    /// Convert to an `alloy_consensus::Block`
    pub fn to_alloy_block<T: Decodable>(&self) -> Result<Block<T>, E2sError> {
        let header: Header = self.header.decode()?;
        let body: BlockBody<T> = self.body.decode()?;

        Ok(Block::new(header, body))
    }

    /// Create from an `alloy_consensus::Block`, attaching the given receipts and total difficulty.
    pub fn from_alloy_block<T: Encodable, R: Encodable>(
        block: &Block<T>,
        receipts: &R,
        total_difficulty: U256,
    ) -> Result<Self, E2sError> {
        let header = CompressedHeader::from_header(&block.header)?;
        let body = CompressedBody::from_body(&block.body)?;

        let compressed_receipts = CompressedSlimReceipts::from_encodable(receipts)?;

        let difficulty = TotalDifficulty::new(total_difficulty);

        Ok(Self::new(header, body)
            .with_receipts(compressed_receipts)
            .with_total_difficulty(difficulty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_header, create_test_receipt, create_test_receipts};
    use alloy_eips::eip4895::Withdrawals;
    use alloy_primitives::{Bytes, U256};
    use reth_ethereum_primitives::{Receipt, TxType};

    #[test]
    fn test_header_conversion_roundtrip() {
        let header = create_header();

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
    fn test_total_difficulty_ssz_le_encoding() {
        // Verify that total-difficulty is encoded as SSZ uint256 (little-endian).
        // See https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md
        let value = U256::from(1u64);
        let td = TotalDifficulty::new(value);
        let entry = td.to_entry();

        // Little-endian: least significant byte first [1, 0, 0, ..., 0]
        assert_eq!(entry.data[0], 1, "First byte must be 1 (little-endian)");
        assert_eq!(entry.data[31], 0, "Last byte must be 0 (little-endian)");
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
        let compressed_receipts = CompressedSlimReceipts::from_rlp(&rlp_data).unwrap();
        let decompressed = compressed_receipts.decompress().unwrap();
        assert_eq!(decompressed, rlp_data);
    }

    #[test]
    fn test_block_tuple_with_data() {
        // Create block with transactions and withdrawals
        let header = create_header();

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

    #[test]
    fn test_block_tuple_component_count() {
        let base = BlockTuple::new(CompressedHeader::new(vec![1]), CompressedBody::new(vec![2]));
        // Mandatory header + body only.
        assert_eq!(base.component_count(), 2);

        // Each optional component bumps the count, up to the 5-component maximum.
        assert_eq!(
            base.clone().with_receipts(CompressedSlimReceipts::new(vec![3])).component_count(),
            3
        );
        let full = base
            .with_receipts(CompressedSlimReceipts::new(vec![3]))
            .with_total_difficulty(TotalDifficulty::new(U256::from(1u64)))
            .with_proof(Proof::new(vec![4]));
        assert_eq!(full.component_count(), 5);
        assert!(full.receipts.is_some() && full.total_difficulty.is_some() && full.proof.is_some());
    }

    #[test]
    fn test_single_receipt_compression_roundtrip() {
        let test_receipt = create_test_receipt(TxType::Eip1559, true, 21000, 2);

        // Compress the receipt
        let compressed_receipts = CompressedSlimReceipts::from_encodable(&test_receipt)
            .expect("Failed to compress receipt");

        // Verify compression
        assert!(!compressed_receipts.data.is_empty());

        // Decode the compressed receipt back
        let decoded_receipt: Receipt =
            compressed_receipts.decode().expect("Failed to decode compressed receipt");

        // Verify that the decoded receipt matches the original
        assert_eq!(decoded_receipt.tx_type, test_receipt.tx_type);
        assert_eq!(decoded_receipt.success, test_receipt.success);
        assert_eq!(decoded_receipt.cumulative_gas_used, test_receipt.cumulative_gas_used);
        assert_eq!(decoded_receipt.logs.len(), test_receipt.logs.len());

        // Verify each log
        for (original_log, decoded_log) in test_receipt.logs.iter().zip(decoded_receipt.logs.iter())
        {
            assert_eq!(decoded_log.address, original_log.address);
            assert_eq!(decoded_log.data.topics(), original_log.data.topics());
        }
    }

    #[test]
    fn test_slim_receipt_matches_spec_rlp() {
        // Spec: CompressedSlimReceipts.data = snappyFramed(rlp([tx-type, status, cumulative-gas,
        // logs])), with no bloom filter. Prove the inner RLP of `EthereumReceipt` is exactly that
        // 4-element list, byte for byte.
        let receipt = create_test_receipt(TxType::Eip1559, true, 21000, 2);

        let compressed = CompressedSlimReceipts::from_encodable(&receipt).unwrap();
        let actual_rlp = compressed.decompress().unwrap();

        // Hand-build rlp([tx-type, status, cumulative-gas, logs]) in spec field order.
        let mut fields = Vec::new();
        (receipt.tx_type as u8).encode(&mut fields);
        receipt.success.encode(&mut fields);
        receipt.cumulative_gas_used.encode(&mut fields);
        receipt.logs.encode(&mut fields);
        let mut expected = Vec::new();
        alloy_rlp::Header { list: true, payload_length: fields.len() }.encode(&mut expected);
        expected.extend_from_slice(&fields);

        assert_eq!(
            actual_rlp, expected,
            "slim receipt RLP must be the 4-element list [tx-type, status, cumulative-gas, logs]"
        );
    }

    #[test]
    fn test_slim_receipts_typed_helpers() {
        // Cover both status variants: post-Byzantium boolean status and a pre-Byzantium 32-byte
        // post-state root, proving a single `SlimReceipt` type round-trips across forks.
        let receipts = vec![
            SlimReceipt {
                tx_type: TxType::Eip1559,
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 21000,
                logs: vec![],
            },
            SlimReceipt {
                tx_type: TxType::Legacy,
                status: Eip658Value::PostState(B256::repeat_byte(0xab)),
                cumulative_gas_used: 42000,
                logs: vec![],
            },
        ];

        let compressed = CompressedSlimReceipts::from_receipts(&receipts).unwrap();
        let decoded = compressed.decode_receipts().unwrap();

        assert_eq!(decoded, receipts);
    }

    #[test]
    fn test_accumulator_from_header_records_known_vectors() {
        // Known-answer vectors computed from the SSZ spec:
        //   hash_tree_root(List[HeaderRecord, 8192])
        let expected_empty: B256 =
            "4a8c3a07c8d23adc5bac61157555c3c784d53d9bc110c1370809bd23cd93777d".parse().unwrap();
        let expected_single_zero: B256 =
            "81fd641249670887a731386e756a7a1538dc781b1b0bf016889045d350812817".parse().unwrap();
        let expected_single_nonzero: B256 =
            "ada35c48d81117f4fd588554cd4c4752356336e84cb41106dea1ceb4cfac8799".parse().unwrap();

        // Empty list
        let acc_empty = Accumulator::from_header_records(&[]).unwrap();
        assert_eq!(acc_empty.root, expected_empty);

        // Single record with zero values
        let records = vec![HeaderRecord { block_hash: B256::ZERO, total_difficulty: U256::ZERO }];
        let acc = Accumulator::from_header_records(&records).unwrap();
        assert_eq!(acc.root, expected_single_zero);

        // Single record with non-zero values
        let records2 = vec![HeaderRecord {
            block_hash: B256::from([1u8; 32]),
            total_difficulty: U256::from(100u64),
        }];
        let acc2 = Accumulator::from_header_records(&records2).unwrap();
        assert_eq!(acc2.root, expected_single_nonzero);
    }

    #[test]
    fn test_accumulator_rejects_oversized_input() {
        let records = vec![
            HeaderRecord { block_hash: B256::ZERO, total_difficulty: U256::ZERO };
            MAX_BLOCKS_PER_ERE + 1
        ];
        assert!(Accumulator::from_header_records(&records).is_err());
    }

    #[test]
    fn test_proof_type_byte_roundtrip() {
        for ty in [
            ProofType::BlockProofHistoricalHashesAccumulator,
            ProofType::BlockProofHistoricalRoots,
            ProofType::BlockProofHistoricalSummariesCapella,
            ProofType::BlockProofHistoricalSummariesDeneb,
        ] {
            assert_eq!(ProofType::from_byte(ty.as_byte()), Some(ty));
        }
        assert_eq!(ProofType::from_byte(4), None);
    }

    #[test]
    fn test_proof_roundtrip() {
        let ssz_proof = vec![0xab; 64];
        let proof = Proof::encode(ProofType::BlockProofHistoricalRoots, &ssz_proof).unwrap();

        // Roundtrip through an Entry
        let entry = proof.to_entry();
        assert_eq!(entry.entry_type, PROOF);
        let recovered = Proof::from_entry(&entry).unwrap();

        let (proof_type, ssz_bytes) = recovered.decode().unwrap();
        assert_eq!(proof_type, ProofType::BlockProofHistoricalRoots);
        assert_eq!(ssz_bytes, ssz_proof);
    }

    #[test]
    fn test_from_entry_rejects_wrong_type() {
        let entry = Entry::new(COMPRESSED_BODY, vec![1, 2, 3]);
        assert!(CompressedHeader::from_entry(&entry).is_err());
    }

    #[test]
    fn test_decode_rejects_trailing_bytes() {
        // A record is exactly `snappyFramed(rlp(...))`; an extra byte after the RLP value
        // must be rejected, not silently ignored.
        let mut rlp = Vec::new();
        7u64.encode(&mut rlp);
        rlp.push(0xff);
        let compressed = CompressedHeader::from_rlp(&rlp).unwrap();
        assert!(compressed.decode::<u64>().is_err());
    }

    #[test]
    fn test_proof_decode_rejects_trailing_bytes() {
        let valid = Proof::encode(ProofType::BlockProofHistoricalRoots, &[1, 2, 3]).unwrap();
        let mut raw = snappy_decompress(&valid.data).unwrap();
        raw.push(0xff); // byte beyond the RLP list
        let tampered = Proof::new(snappy_compress(&raw).unwrap());
        assert!(tampered.decode().is_err());
    }

    #[test]
    fn test_proof_decode_rejects_extra_list_item() {
        // Build rlp([proof-type, ssz, extra]) — a third item must be rejected.
        let mut payload = Vec::new();
        0u8.encode(&mut payload);
        alloy_primitives::Bytes::from(vec![1, 2, 3]).encode(&mut payload);
        99u8.encode(&mut payload);
        let mut rlp = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut rlp);
        rlp.extend_from_slice(&payload);
        let proof = Proof::new(snappy_compress(&rlp).unwrap());
        assert!(proof.decode().is_err());
    }

    #[test]
    fn test_receipt_list_compression() {
        let receipts = create_test_receipts();

        // Compress the list of receipts
        let compressed_receipts = CompressedSlimReceipts::from_encodable_list(&receipts)
            .expect("Failed to compress receipt list");

        // Decode the compressed receipts back. ERE always stores slim receipts (no bloom), so the
        // bare `Receipt` (`alloy_consensus::EthereumReceipt`) is the canonical decode target.
        let decoded_receipts: Vec<Receipt> =
            compressed_receipts.decode().expect("Failed to decode compressed receipt list");

        // Verify that the decoded receipts match the original
        assert_eq!(decoded_receipts.len(), receipts.len());

        for (original, decoded) in receipts.iter().zip(decoded_receipts.iter()) {
            assert_eq!(decoded.tx_type, original.tx_type);
            assert_eq!(decoded.success, original.success);
            assert_eq!(decoded.cumulative_gas_used, original.cumulative_gas_used);
            assert_eq!(decoded.logs.len(), original.logs.len());

            for (original_log, decoded_log) in original.logs.iter().zip(decoded.logs.iter()) {
                assert_eq!(decoded_log.address, original_log.address);
                assert_eq!(decoded_log.data.topics(), original_log.data.topics());
            }
        }
    }
}
