//! Storage abstractions and implementations for EIP-7928 Block Access Lists (BALs).

use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use std::io;

pub mod disk;
pub use disk::{DiskFileBalStore, DiskFileBalStoreConfig, DEFAULT_MAX_BAL_STORE_ENTRIES};

/// A store for EIP-7928 Block Access Lists (BALs).
///
/// The store is keyed by block hash and maintains a block-number index for range queries.
/// Implementations should preserve contiguous-range semantics:
/// queries by range stop at the first missing block.
pub trait BalStore: Send + Sync + 'static {
    /// Inserts a BAL for the given block hash and number.
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError>;

    /// Returns BALs for each requested block hash in the same order.
    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> Result<Vec<Option<Bytes>>, BalStoreError>;

    /// Returns contiguous BALs in `[start, start + count)` until the first gap.
    fn get_by_range(&self, start: BlockNumber, count: u64) -> Result<Vec<Bytes>, BalStoreError>;
}

/// Error variants that can occur when interacting with a BAL store.
#[derive(Debug, thiserror::Error)]
pub enum BalStoreError {
    /// Filesystem I/O error.
    #[error("BAL store I/O error: {0}")]
    Io(#[from] io::Error),
    /// Other implementation-specific error.
    #[error(transparent)]
    Other(Box<dyn core::error::Error + Send + Sync>),
}
