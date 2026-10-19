//! Snappy-framed compression helpers and a generic Snappy + RLP codec.
//!
//! These utilities are format-agnostic and shared across the e2store-based file formats.

use crate::e2s::error::E2sError;
use alloy_rlp::{Decodable, Encodable};
use snap::{read::FrameDecoder, write::FrameEncoder};
use std::{
    io::{Read, Write},
    marker::PhantomData,
};

/// Compress raw bytes with Snappy framed encoding.
pub fn snappy_compress(data: &[u8]) -> Result<Vec<u8>, E2sError> {
    let mut compressed = Vec::new();
    {
        let mut encoder = FrameEncoder::new(&mut compressed);
        Write::write_all(&mut encoder, data)
            .map_err(|e| E2sError::SnappyCompression(format!("Failed to compress: {e}")))?;
        encoder
            .flush()
            .map_err(|e| E2sError::SnappyCompression(format!("Failed to flush encoder: {e}")))?;
    }
    Ok(compressed)
}

/// Decompress Snappy framed-encoded bytes.
pub fn snappy_decompress(data: &[u8]) -> Result<Vec<u8>, E2sError> {
    let mut decoder = FrameDecoder::new(data);
    let mut decompressed = Vec::new();
    Read::read_to_end(&mut decoder, &mut decompressed)
        .map_err(|e| E2sError::SnappyDecompression(format!("Failed to decompress: {e}")))?;
    Ok(decompressed)
}

/// Generic codec for Snappy-framed-compressed RLP data.
#[derive(Debug, Clone, Default)]
pub struct SnappyRlpCodec<T> {
    _phantom: PhantomData<T>,
}

impl<T> SnappyRlpCodec<T> {
    /// Create a new codec for the given type.
    pub const fn new() -> Self {
        Self { _phantom: PhantomData }
    }
}

impl<T: Decodable> SnappyRlpCodec<T> {
    /// Decode compressed data into the target type.
    ///
    /// A record holds exactly one RLP value, so any bytes left after it are treated as corruption
    /// and rejected rather than silently ignored.
    pub fn decode(&self, compressed_data: &[u8]) -> Result<T, E2sError> {
        let decompressed = snappy_decompress(compressed_data)?;
        let mut slice = decompressed.as_slice();
        let value = T::decode(&mut slice)
            .map_err(|e| E2sError::Rlp(format!("Failed to decode RLP data: {e}")))?;
        if !slice.is_empty() {
            return Err(E2sError::Rlp(format!(
                "Trailing bytes after RLP value: {} byte(s) remain",
                slice.len()
            )));
        }
        Ok(value)
    }
}

impl<T: Encodable> SnappyRlpCodec<T> {
    /// Encode data into compressed format.
    pub fn encode(&self, data: &T) -> Result<Vec<u8>, E2sError> {
        let mut rlp_data = Vec::new();
        data.encode(&mut rlp_data);
        snappy_compress(&rlp_data)
    }
}
