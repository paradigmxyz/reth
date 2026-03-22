//! Compressed data decoding utilities.

use crate::e2s::error::E2sError;
use alloy_rlp::Decodable;

/// Extension trait for generic decoding from compressed data
pub trait DecodeCompressedRlp {
    /// Decompress and decode the data into the given type
    fn decode<T: Decodable>(&self) -> Result<T, E2sError>;
}
