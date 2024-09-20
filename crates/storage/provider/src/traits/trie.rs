use std::collections::HashMap;

use alloy_primitives::B256;
use auto_impl::auto_impl;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::updates::{StorageTrieUpdates, TrieUpdates};

/// Trie Writer
#[auto_impl(&, Arc, Box)]
pub trait TrieWriter: Send + Sync {
    /// Writes trie updates to the database.
    ///
    /// Returns the number of entries modified.
    fn write_trie_updates(&self, trie_updates: &TrieUpdates) -> ProviderResult<usize>;
}

/// Storage Trie Writer
#[auto_impl(&, Arc, Box)]
pub trait StorageTrieWriter: Send + Sync {
    /// Writes storage trie updates from the given storage trie map.
    ///
    /// First sorts the storage trie updates by the hashed address key, writing in sorted order.
    ///
    /// Returns the number of entries modified.
    fn write_storage_trie_updates(
        &self,
        storage_tries: &HashMap<B256, StorageTrieUpdates>,
    ) -> ProviderResult<usize>;

    /// Writes storage trie updates for the given hashed address.
    fn write_individual_storage_trie_updates(
        &self,
        hashed_address: B256,
        updates: &StorageTrieUpdates,
    ) -> ProviderResult<usize>;
}
