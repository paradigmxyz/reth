//! Compressed data decoding utilities.

use crate::e2s::error::E2sError;
use alloy_rlp::Decodable;
use ssz::Decode;

/// Extension trait for generic decoding from compressed data
pub trait DecodeCompressed {
    /// Decompress and decode the data into the given type
    fn decode<T: Decodable>(&self) -> Result<T, E2sError>;
}

/// Extension trait for generic decoding from compressed ssz data
pub trait DecodeCompressedSsz {
    /// Decompress and decode the SSZ data into the given type
    fn decode<T: Decode>(&self) -> Result<T, E2sError>;
}
