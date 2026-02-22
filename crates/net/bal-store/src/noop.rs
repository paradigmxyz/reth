//! No-op BAL store implementation.

use crate::{BalStore, BalStoreError};
use alloy_primitives::{BlockHash, BlockNumber, Bytes};

/// A no-op BAL store.
///
/// This implementation never persists data and always returns cache-miss semantics:
/// hash lookups return `None` and range lookups return an empty result.
#[derive(Clone, Debug, Default)]
pub struct NoopBalStore;

impl BalStore for NoopBalStore {
    fn insert(
        &self,
        _block_hash: BlockHash,
        _block_number: BlockNumber,
        _bal: Bytes,
    ) -> Result<(), BalStoreError> {
        Ok(())
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> Result<Vec<Option<Bytes>>, BalStoreError> {
        Ok(vec![None; block_hashes.len()])
    }

    fn get_by_range(&self, _start: BlockNumber, _count: u64) -> Result<Vec<Bytes>, BalStoreError> {
        Ok(Vec::new())
    }
}
