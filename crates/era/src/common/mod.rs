//! Common utilities and shared functionality.

pub mod compression;
pub mod decode;
pub mod file_ops;

/// Maximum number of entries per e2store era file.
///
/// One value for every era format: `.era` files store one beacon block per slot, and
/// `.ere`/`.era1` files cap their block count at the same number so the header-record accumulator
/// (`List[HeaderRecord, 8192]`) stays within its SSZ size limit. Numerically this is the beacon
/// chain `SLOTS_PER_HISTORICAL_ROOT`.
pub const MAX_ENTRIES_PER_ERA: u64 = 8192;
