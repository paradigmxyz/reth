//! Commonly used constants for transaction types.

/// Identifier parameter for legacy transaction
pub const COMPACT_IDENTIFIER_LEGACY: usize = 0;

/// Identifier parameter for EIP-2930 transaction
pub const COMPACT_IDENTIFIER_EIP2930: usize = 1;

/// Identifier parameter for EIP-1559 transaction
pub const COMPACT_IDENTIFIER_EIP1559: usize = 2;

/// For backwards compatibility purposes only 2 bits of the type are encoded in the identifier
/// parameter. In the case of a [`COMPACT_EXTENDED_IDENTIFIER_FLAG`], the full transaction type is
/// read from the buffer as a single byte.
pub const COMPACT_EXTENDED_IDENTIFIER_FLAG: usize = 3;
