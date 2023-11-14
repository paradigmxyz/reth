//! KZG type bindings

use alloy_primitives::FixedBytes;

/// How many bytes are in a blob
pub const BYTES_PER_BLOB: usize = 131072;

/// A Blob serialized as 0x-prefixed hex string
pub type Blob = FixedBytes<BYTES_PER_BLOB>;

/// A commitment/proof serialized as 0x-prefixed hex string
pub type Bytes48 = FixedBytes<48>;
