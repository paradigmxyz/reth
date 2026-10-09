//! Validated original bytecode and metadata for opt-in chunk storage.

use alloc::vec::Vec;
use alloy_primitives::{keccak256, Bytes, B256};
use core::{fmt, ops::Range};

/// Original payload size, excluding execution-only padding.
pub const CODE_CHUNK_SIZE: usize = 24 * 1024 - 35;
/// Maximum unchanged historical single-record payload length.
pub const LEGACY_CODE_CHUNK_SIZE: usize = 24 * 1024;
/// Maximum number of original payloads.
pub const MAX_CODE_CHUNKS: usize = 40;
/// Maximum original runtime length.
pub const MAX_CODE_SIZE: usize = CODE_CHUNK_SIZE * MAX_CODE_CHUNKS;

/// Authenticated runtime bytes. Construction never changes the submitted bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedCode {
    original: Bytes,
    hash: B256,
    chunks: Vec<Bytes>,
    descriptor: Option<CodeChunkDescriptor>,
}

impl ValidatedCode {
    /// Validate runtime boundaries and construct shared slices of the original input.
    /// Single-chunk code retains legacy truncated-PUSH semantics.
    pub fn new(original: Bytes) -> Result<Self, CodeValidationError> {
        if original.len() > MAX_CODE_SIZE {
            return Err(CodeValidationError::CodeTooLarge {
                actual: original.len(),
                maximum: MAX_CODE_SIZE,
            });
        }
        let hash = keccak256(&original);
        let chunks = (0..original.len())
            .step_by(CODE_CHUNK_SIZE)
            .map(|start| original.slice(start..(start + CODE_CHUNK_SIZE).min(original.len())))
            .collect::<Vec<_>>();
        let descriptor = if chunks.len() > 1 {
            Some(CodeChunkDescriptor::with_preparation(
                original.len() as u32,
                chunks.iter().map(keccak256).collect(),
                prepare_chunks(&original),
            )?)
        } else {
            None
        };
        Ok(Self { original, hash, chunks, descriptor })
    }

    /// Authenticate imported ordered payloads before they can be persisted.
    pub fn from_chunks(
        hash: B256,
        size: u32,
        hashes: Vec<B256>,
        chunks: Vec<Bytes>,
    ) -> Result<Self, CodeValidationError> {
        let descriptor = CodeChunkDescriptor::new(size, hashes)?;
        if chunks.len() != descriptor.chunk_hashes.len() {
            return Err(CodeValidationError::PayloadCount {
                expected: descriptor.chunk_hashes.len(),
                actual: chunks.len(),
            });
        }
        let mut original = Vec::with_capacity(size as usize);
        for (index, chunk) in chunks.iter().enumerate() {
            let expected = descriptor.chunk_range(index as u32).expect("validated index").len();
            if chunk.len() != expected {
                return Err(CodeValidationError::ChunkLength {
                    index: index as u32,
                    expected,
                    actual: chunk.len(),
                });
            }
            if keccak256(chunk) != descriptor.chunk_hashes[index] {
                return Err(CodeValidationError::ChunkHash { index: index as u32 });
            }
            original.extend_from_slice(chunk);
        }
        if keccak256(&original) != hash {
            return Err(CodeValidationError::FullCodeHash);
        }
        Self::new(original.into())
    }

    /// Original unpadded bytes.
    pub fn original_bytes(&self) -> &Bytes {
        &self.original
    }
    /// Hash of all original bytes.
    pub const fn code_hash(&self) -> B256 {
        self.hash
    }
    /// Original payloads; empty code has none.
    pub fn chunks(&self) -> &[Bytes] {
        &self.chunks
    }
    /// Metadata exists only for multi-chunk code.
    pub const fn descriptor(&self) -> Option<&CodeChunkDescriptor> {
        self.descriptor.as_ref()
    }
}

/// Size and ordered commitments, independent of any chain account-extension layout.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CodeChunkDescriptor {
    code_size: u32,
    chunk_hashes: Vec<B256>,
    preparation: Vec<ChunkPreparation>,
}

impl CodeChunkDescriptor {
    /// Validate size and exact hash count for an account commitment.
    /// Execution context is placeholder data; persist only a descriptor derived by
    /// `ValidatedCode::new` or a fully authenticated ingestion path.
    pub fn new(code_size: u32, chunk_hashes: Vec<B256>) -> Result<Self, CodeValidationError> {
        if code_size as usize <= CODE_CHUNK_SIZE || code_size as usize > MAX_CODE_SIZE {
            return Err(CodeValidationError::InvalidCodeSize { size: code_size });
        }
        let expected = (code_size as usize).div_ceil(CODE_CHUNK_SIZE);
        if chunk_hashes.len() != expected {
            return Err(CodeValidationError::HashCount { expected, actual: chunk_hashes.len() });
        }
        let preparation = (0..expected)
            .map(|index| ChunkPreparation {
                leading_data_len: 0,
                jump_data_len: 0,
                lookahead: alloc::vec![0; (code_size as usize - ((index + 1) * CODE_CHUNK_SIZE).min(code_size as usize)).min(32)].into(),
                next_chunk: (index + 1 < expected).then_some((index + 1) as u32),
            })
            .collect();
        Ok(Self { code_size, chunk_hashes, preparation })
    }

    /// Construct authenticated context, keyed with this descriptor's global code hash.
    pub fn with_preparation(
        code_size: u32,
        chunk_hashes: Vec<B256>,
        preparation: Vec<ChunkPreparation>,
    ) -> Result<Self, CodeValidationError> {
        let mut descriptor = Self::new(code_size, chunk_hashes)?;
        if preparation.len() != descriptor.chunk_hashes.len() {
            return Err(CodeValidationError::DescriptorEncoding);
        }
        for (index, prepared) in preparation.iter().enumerate() {
            let payload_len = descriptor.chunk_range(index as u32).unwrap().len();
            if prepared.leading_data_len as usize > payload_len.min(32) ||
                prepared.jump_data_len as usize > payload_len.min(32) ||
                prepared.lookahead.len() !=
                    (code_size as usize -
                        ((index + 1) * CODE_CHUNK_SIZE).min(code_size as usize))
                    .min(32) ||
                prepared.next_chunk.is_some_and(|next| {
                    next != index as u32 + 1 || next as usize >= preparation.len()
                }) ||
                payload_len +
                    prepared.lookahead.len() +
                    if prepared.next_chunk.is_some() { 3 } else { 1 } >
                    LEGACY_CODE_CHUNK_SIZE
            {
                return Err(CodeValidationError::DescriptorEncoding);
            }
        }
        descriptor.preparation = preparation;
        Ok(descriptor)
    }

    /// Context for one prepared execution buffer; raw payloads remain unchanged.
    pub fn preparation(&self, index: u32) -> Option<&ChunkPreparation> {
        self.preparation.get(index as usize)
    }

    /// Compare only the size and hashes committed by account metadata.
    /// Preparation is derived from authenticated original code at ingestion.
    pub fn same_commitment(&self, other: &Self) -> bool {
        self.code_size == other.code_size && self.chunk_hashes == other.chunk_hashes
    }

    /// Committed original size.
    pub const fn code_size(&self) -> u32 {
        self.code_size
    }
    /// Ordered payload commitments. Repeated commitments are allowed.
    pub fn chunk_hashes(&self) -> &[B256] {
        &self.chunk_hashes
    }
    /// Original byte range; invalid indices have no range.
    pub fn chunk_range(&self, index: u32) -> Option<Range<usize>> {
        if index as usize >= self.chunk_hashes.len() {
            return None;
        }
        let start = (index as usize).checked_mul(CODE_CHUNK_SIZE)?;
        let end = start.checked_add(CODE_CHUNK_SIZE)?.min(self.code_size as usize);
        Some(start..end)
    }
}

/// Invalid untrusted runtime or metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeValidationError {
    /// Runtime exceeds the opt-in limit.
    CodeTooLarge { actual: usize, maximum: usize },
    /// Descriptor does not describe multi-chunk code.
    InvalidCodeSize { size: u32 },
    /// Incorrect commitment count.
    HashCount { expected: usize, actual: usize },
    /// Incorrect payload count.
    PayloadCount { expected: usize, actual: usize },
    /// Payload has an incorrect derived length.
    ChunkLength { index: u32, expected: usize, actual: usize },
    /// Payload does not match its commitment.
    ChunkHash { index: u32 },
    /// Concatenated bytes do not match the full identity.
    FullCodeHash,
    /// Persisted descriptor framing is invalid.
    DescriptorEncoding,
}

impl fmt::Display for CodeValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid chunked code: {self:?}")
    }
}

impl core::error::Error for CodeValidationError {}

/// Authenticated execution-only overlap and transition data for one original slice.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ChunkPreparation {
    /// Leading original immediate bytes replaced by valid JUMPDESTs in the execution view.
    pub leading_data_len: u8,
    /// Leading bytes skipped by original PUSH-only global jump analysis.
    pub jump_data_len: u8,
    /// Up to 32 original bytes after this slice, for every possible legal entry path.
    pub lookahead: Bytes,
    /// Next slice for a fused stack-neutral transfer, or final STOP.
    pub next_chunk: Option<u32>,
}

fn prepare_chunks(code: &[u8]) -> Vec<ChunkPreparation> {
    let count = code.len().div_ceil(CODE_CHUNK_SIZE);
    let mut prepared = (0..count)
        .map(|index| ChunkPreparation {
            leading_data_len: 0,
            jump_data_len: 0,
            lookahead: Bytes::copy_from_slice(
                &code[((index + 1) * CODE_CHUNK_SIZE).min(code.len())..
                    ((index + 1) * CODE_CHUNK_SIZE + 32).min(code.len())],
            ),
            next_chunk: (index + 1 < count).then_some((index + 1) as u32),
        })
        .collect::<Vec<_>>();
    let mut pc = 0;
    while pc < code.len() {
        let width = match code[pc] {
            0x60..=0x7f => (code[pc] - 0x5f) as usize,
            0xe6..=0xe8 => 1,
            _ => 0,
        };
        let end = pc + width + 1;
        let index = pc / CODE_CHUNK_SIZE;
        let boundary = ((index + 1) * CODE_CHUNK_SIZE).min(code.len());
        if end > boundary {
            let available = end.min(code.len()) - boundary;
            if index + 1 < count {
                prepared[index + 1].leading_data_len = available as u8;
            }
        }
        if end >= code.len() {
            prepared[index].next_chunk = None;
        }
        pc = end;
    }
    let mut pc = 0;
    while pc < code.len() {
        let width = if (0x60..=0x7f).contains(&code[pc]) { (code[pc] - 0x5f) as usize } else { 0 };
        let end = pc + width + 1;
        let boundary = (pc / CODE_CHUNK_SIZE + 1) * CODE_CHUNK_SIZE;
        if end > boundary && boundary < code.len() {
            prepared[boundary / CODE_CHUNK_SIZE].jump_data_len =
                (end.min(code.len()) - boundary) as u8;
        }
        pc = end;
    }
    prepared
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compress for CodeChunkDescriptor {
    type Compressed = bytes::BytesMut;

    fn compress(self) -> Self::Compressed {
        let mut output = bytes::BytesMut::with_capacity(5 + self.chunk_hashes.len() * 68);
        self.compress_to_buf(&mut output);
        output
    }

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        buf.put_u8(1);
        buf.put_slice(&self.code_size.to_be_bytes());
        for (hash, prepared) in self.chunk_hashes.iter().zip(&self.preparation) {
            buf.put_slice(hash.as_slice());
            buf.put_u8(prepared.leading_data_len);
            buf.put_u8(prepared.jump_data_len);
            buf.put_u8(prepared.lookahead.len() as u8);
            buf.put_u8(prepared.next_chunk.map_or(0xff, |next| next as u8));
            buf.put_slice(&prepared.lookahead);
        }
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Decompress for CodeChunkDescriptor {
    fn decompress(value: &[u8]) -> Result<Self, reth_codecs::DecompressError> {
        let invalid = || reth_codecs::DecompressError::new(CodeValidationError::DescriptorEncoding);
        if value.first() != Some(&1) {
            return Err(invalid());
        }
        let header = value.get(1..5).ok_or_else(invalid)?;
        let size = u32::from_be_bytes(header.try_into().map_err(|_| invalid())?);
        if size as usize <= CODE_CHUNK_SIZE || size as usize > MAX_CODE_SIZE {
            return Err(invalid());
        }
        let count = (size as usize).div_ceil(CODE_CHUNK_SIZE);
        let mut hashes = Vec::with_capacity(count);
        let mut preparation = Vec::with_capacity(count);
        let mut rest = &value[5..];
        for _ in 0..count {
            let entry = rest.get(..36).ok_or_else(invalid)?;
            hashes.push(B256::from_slice(&entry[..32]));
            let spill = entry[34] as usize;
            let lookahead = rest.get(36..36 + spill).ok_or_else(invalid)?;
            preparation.push(ChunkPreparation {
                leading_data_len: entry[32],
                jump_data_len: entry[33],
                lookahead: Bytes::copy_from_slice(lookahead),
                next_chunk: (entry[35] != 0xff).then_some(entry[35] as u32),
            });
            rest = &rest[36 + spill..];
        }
        if !rest.is_empty() {
            return Err(invalid());
        }
        Self::with_preparation(size, hashes, preparation).map_err(reth_codecs::DecompressError::new)
    }
}

/// Account-committed representation supplied by the chain's extension decoder.
/// Absence of chunk metadata means legacy code, not empty code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeRepresentation {
    /// Canonical empty code.
    Empty,
    /// Nonempty original bytecode stored in the legacy table.
    Legacy,
    /// Multi-chunk code with account-committed size and ordered hashes.
    Chunked(CodeChunkDescriptor),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "reth-codec")]
    use reth_codecs::{Compress, Decompress};

    #[test]
    fn every_push_overlap_retains_raw_values_and_bounded_context() {
        for width in 1..=32 {
            for before in 0..width {
                let mut raw = alloc::vec![0; CODE_CHUNK_SIZE * 2];
                raw[CODE_CHUNK_SIZE - before - 1] = 0x5f + width as u8;
                raw[CODE_CHUNK_SIZE..CODE_CHUNK_SIZE + width - before].fill(0xa7);
                let code = ValidatedCode::new(raw.clone().into()).unwrap();
                let descriptor = code.descriptor().unwrap();
                let first = descriptor.preparation(0).unwrap();
                let second = descriptor.preparation(1).unwrap();
                assert_eq!(&first.lookahead[..width - before], alloc::vec![0xa7; width - before]);
                assert_eq!(first.lookahead.len(), 32);
                assert_eq!(second.leading_data_len as usize, width - before);
                assert_eq!(code.chunks().concat(), raw);
                assert!(CODE_CHUNK_SIZE + first.lookahead.len() + 3 <= LEGACY_CODE_CHUNK_SIZE);
            }
        }
    }

    #[test]
    fn lookahead_only_final_chunk_stops_previous_segment() {
        let mut raw = alloc::vec![0; CODE_CHUNK_SIZE + 3];
        raw[CODE_CHUNK_SIZE - 1] = 0x7f;
        raw[CODE_CHUNK_SIZE..].copy_from_slice(&[0x5b, 0x60, 0x56]);
        let code = ValidatedCode::new(raw.into()).unwrap();
        let descriptor = code.descriptor().unwrap();
        assert_eq!(descriptor.preparation(0).unwrap().next_chunk, None);
        assert_eq!(descriptor.preparation(0).unwrap().lookahead.len(), 3);
        assert_eq!(&descriptor.preparation(0).unwrap().lookahead[..3], &[0x5b, 0x60, 0x56]);
        assert_eq!(descriptor.preparation(1).unwrap().leading_data_len, 3);
        assert_eq!(descriptor.preparation(1).unwrap().next_chunk, None);
    }

    #[test]
    fn logical_limit_is_exactly_forty_slices() {
        let code = ValidatedCode::new(alloc::vec![0; MAX_CODE_SIZE].into()).unwrap();
        assert_eq!(code.chunks().len(), 40);
        assert_eq!(MAX_CODE_SIZE, 981640);
        assert!(ValidatedCode::new(alloc::vec![0; MAX_CODE_SIZE + 1].into()).is_err());
    }

    #[cfg(feature = "reth-codec")]
    #[test]
    fn preparation_codec_rejects_trailing_and_truncated_records() {
        let mut raw = alloc::vec![0; CODE_CHUNK_SIZE + 7];
        raw[CODE_CHUNK_SIZE - 1] = 0x7f;
        let code = ValidatedCode::new(raw.into()).unwrap();
        let descriptor = code.descriptor().unwrap();
        let encoded = descriptor.clone().compress();
        assert_eq!(CodeChunkDescriptor::decompress(&encoded).unwrap(), *descriptor);
        for length in 0..encoded.len() {
            assert!(CodeChunkDescriptor::decompress(&encoded[..length]).is_err());
        }
        let mut trailing = encoded.to_vec();
        trailing.push(0);
        assert!(CodeChunkDescriptor::decompress(&trailing).is_err());
    }
}
