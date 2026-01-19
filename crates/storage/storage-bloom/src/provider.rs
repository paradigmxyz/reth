//! State provider wrapper with bloom filter integration.

use alloy_primitives::{Address, StorageKey, StorageValue};
use reth_storage_errors::provider::ProviderResult;
use std::sync::Arc;

use crate::StorageBloomFilter;

/// Extension trait for state providers that support bloom filter optimization.
pub trait StorageBloomProvider {
    /// Get the storage bloom filter, if available.
    fn storage_bloom(&self) -> Option<&Arc<StorageBloomFilter>>;

    /// Check storage with bloom filter optimization.
    ///
    /// If bloom filter is available and says slot is definitely empty,
    /// returns `Ok(None)` without hitting the database.
    ///
    /// Otherwise, falls back to normal storage lookup.
    fn storage_with_bloom(
        &self,
        address: Address,
        storage_key: StorageKey,
        fallback: impl FnOnce() -> ProviderResult<Option<StorageValue>>,
    ) -> ProviderResult<Option<StorageValue>> {
        if let Some(bloom) = self.storage_bloom() {
            // Check bloom filter first
            if !bloom.maybe_contains(address, storage_key) {
                // Definitely not present - return None without DB lookup
                return Ok(None);
            }

            // Maybe present - need to check DB
            let result = fallback()?;

            // Track false positives for metrics
            if result.is_none() || result == Some(StorageValue::ZERO) {
                bloom.record_false_positive();
            }

            Ok(result)
        } else {
            // No bloom filter - use normal path
            fallback()
        }
    }
}
