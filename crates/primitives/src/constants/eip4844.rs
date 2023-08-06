//! [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#parameters) protocol constants for shard Blob Transactions.

/// Size a single field element in bytes.
pub const FIELD_ELEMENT_BYTES: u64 = 32;

/// How many field elements are stored in a single data blob.
pub const FIELD_ELEMENTS_PER_BLOB: u64 = 4096;

/// Gas consumption of a single data blob.
pub const DATA_GAS_PER_BLOB: u64 = 131_072u64; // 32*4096 = 131072 == 2^17 == 0x20000

/// Maximum data gas for data blobs in a single block.
pub const MAX_DATA_GAS_PER_BLOCK: u64 = 786_432u64; // 0xC0000

/// Target data gas for data blobs in a single block.
pub const TARGET_DATA_GAS_PER_BLOCK: u64 = 393_216u64; // 0x60000

/// Maximum number of data blobs in a single block.
pub const MAX_BLOBS_PER_BLOCK: u64 = MAX_DATA_GAS_PER_BLOCK / DATA_GAS_PER_BLOB; // 786432 / 131072  = 6

/// Target number of data blobs in a single block.
pub const TARGET_BLOBS_PER_BLOCK: u64 = TARGET_DATA_GAS_PER_BLOCK / DATA_GAS_PER_BLOB; // 393216 / 131072 = 3

/// Used to determine the price for next data blob
pub const BLOB_GASPRICE_UPDATE_FRACTION: u64 = 3_338_477u64; // 3338477
