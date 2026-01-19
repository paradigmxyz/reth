//! Bloom filter-aware state provider wrapper.
//!
//! This module provides a state provider wrapper that uses a bloom filter to
//! short-circuit storage reads for empty slots.

use crate::StateProvider;
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateRootProvider, StorageRootProvider,
};
use reth_storage_bloom::{StorageBloomFilter, StorageBloomProvider};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, TrieInput,
};
use revm_database::BundleState;
use std::sync::Arc;

/// A state provider wrapper that uses a bloom filter to optimize storage reads.
///
/// When reading storage, this provider first checks the bloom filter:
/// - If the bloom filter says the slot is definitely empty, returns `None` without DB access
/// - If the bloom filter says the slot might exist, proceeds with normal DB lookup
#[derive(Debug)]
pub struct BloomStateProvider<P> {
    /// The underlying state provider.
    inner: P,
    /// The storage bloom filter.
    bloom: Arc<StorageBloomFilter>,
}

impl<P> BloomStateProvider<P> {
    /// Create a new bloom-aware state provider.
    pub const fn new(inner: P, bloom: Arc<StorageBloomFilter>) -> Self {
        Self { inner, bloom }
    }

    /// Get the underlying provider.
    pub const fn inner(&self) -> &P {
        &self.inner
    }

    /// Get the bloom filter.
    pub const fn bloom(&self) -> &Arc<StorageBloomFilter> {
        &self.bloom
    }
}

impl<P> StorageBloomProvider for BloomStateProvider<P> {
    fn storage_bloom(&self) -> Option<&Arc<StorageBloomFilter>> {
        Some(&self.bloom)
    }
}

impl<P: AccountReader> AccountReader for BloomStateProvider<P> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.inner.basic_account(address)
    }
}

impl<P: BlockHashReader> BlockHashReader for BloomStateProvider<P> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.inner.canonical_hashes_range(start, end)
    }
}

impl<P: BytecodeReader> BytecodeReader for BloomStateProvider<P> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.inner.bytecode_by_hash(code_hash)
    }
}

impl<P: StateRootProvider> StateRootProvider for BloomStateProvider<P> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.inner.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.inner.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_from_nodes_with_updates(input)
    }
}

impl<P: StorageRootProvider> StorageRootProvider for BloomStateProvider<P> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.inner.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        self.inner.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.inner.storage_multiproof(address, slots, hashed_storage)
    }
}

impl<P: StateProofProvider> StateProofProvider for BloomStateProvider<P> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.inner.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.inner.multiproof(input, targets)
    }

    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        self.inner.witness(input, target)
    }
}

impl<P: HashedPostStateProvider> HashedPostStateProvider for BloomStateProvider<P> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        self.inner.hashed_post_state(bundle_state)
    }
}

impl<P: StateProvider> StateProvider for BloomStateProvider<P> {
    /// Get storage with bloom filter optimization.
    ///
    /// If the bloom filter indicates the slot is definitely empty,
    /// returns `None` without hitting the database.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        // Check bloom filter first
        if !self.bloom.maybe_contains(account, storage_key) {
            // Definitely not present - return None without DB lookup
            return Ok(None);
        }

        // Maybe present - need to check DB
        let result = self.inner.storage(account, storage_key)?;

        // Track false positives for metrics
        if result.is_none() || result == Some(StorageValue::ZERO) {
            self.bloom.record_false_positive();
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_storage_bloom::StorageBloomConfig;

    /// Mock state provider for testing.
    struct MockStateProvider {
        storage_calls: std::sync::atomic::AtomicUsize,
    }

    impl MockStateProvider {
        fn new() -> Self {
            Self { storage_calls: std::sync::atomic::AtomicUsize::new(0) }
        }

        fn storage_call_count(&self) -> usize {
            self.storage_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl AccountReader for MockStateProvider {
        fn basic_account(&self, _: &Address) -> ProviderResult<Option<Account>> {
            Ok(None)
        }
    }

    impl BlockHashReader for MockStateProvider {
        fn block_hash(&self, _: u64) -> ProviderResult<Option<B256>> {
            Ok(None)
        }
        fn canonical_hashes_range(
            &self,
            _: BlockNumber,
            _: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok(vec![])
        }
    }

    impl BytecodeReader for MockStateProvider {
        fn bytecode_by_hash(&self, _: &B256) -> ProviderResult<Option<Bytecode>> {
            Ok(None)
        }
    }

    impl StateRootProvider for MockStateProvider {
        fn state_root(&self, _: HashedPostState) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }
        fn state_root_from_nodes(&self, _: TrieInput) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }
        fn state_root_with_updates(
            &self,
            _: HashedPostState,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }
        fn state_root_from_nodes_with_updates(
            &self,
            _: TrieInput,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }
    }

    impl StorageRootProvider for MockStateProvider {
        fn storage_root(&self, _: Address, _: HashedStorage) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }
        fn storage_proof(
            &self,
            _: Address,
            _: B256,
            _: HashedStorage,
        ) -> ProviderResult<reth_trie::StorageProof> {
            unimplemented!()
        }
        fn storage_multiproof(
            &self,
            _: Address,
            _: &[B256],
            _: HashedStorage,
        ) -> ProviderResult<StorageMultiProof> {
            unimplemented!()
        }
    }

    impl StateProofProvider for MockStateProvider {
        fn proof(&self, _: TrieInput, _: Address, _: &[B256]) -> ProviderResult<AccountProof> {
            unimplemented!()
        }
        fn multiproof(&self, _: TrieInput, _: MultiProofTargets) -> ProviderResult<MultiProof> {
            unimplemented!()
        }
        fn witness(&self, _: TrieInput, _: HashedPostState) -> ProviderResult<Vec<Bytes>> {
            Ok(Default::default())
        }
    }

    impl HashedPostStateProvider for MockStateProvider {
        fn hashed_post_state(&self, _: &BundleState) -> HashedPostState {
            HashedPostState::default()
        }
    }

    impl StateProvider for MockStateProvider {
        fn storage(&self, _: Address, _: StorageKey) -> ProviderResult<Option<StorageValue>> {
            self.storage_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None) // Always return empty for test
        }
    }

    #[test]
    fn test_bloom_skips_db_for_empty_slots() {
        let config =
            StorageBloomConfig { expected_items: 1000, false_positive_rate: 0.01, enabled: true };
        let bloom = Arc::new(StorageBloomFilter::new(config));
        let mock = MockStateProvider::new();
        let provider = BloomStateProvider::new(mock, bloom);

        let addr = Address::repeat_byte(0x42);
        let slot = B256::repeat_byte(0x01);

        // Slot not in bloom - should skip DB
        let result = provider.storage(addr, slot).unwrap();
        assert_eq!(result, None);
        assert_eq!(provider.inner().storage_call_count(), 0);
    }

    #[test]
    fn test_bloom_checks_db_for_maybe_present() {
        let config =
            StorageBloomConfig { expected_items: 1000, false_positive_rate: 0.01, enabled: true };
        let bloom = Arc::new(StorageBloomFilter::new(config));

        let addr = Address::repeat_byte(0x42);
        let slot = B256::repeat_byte(0x01);

        // Insert into bloom first
        bloom.insert(addr, slot);

        let mock = MockStateProvider::new();
        let provider = BloomStateProvider::new(mock, bloom);

        // Slot in bloom - should check DB
        let result = provider.storage(addr, slot).unwrap();
        assert_eq!(result, None); // Mock returns None
        assert_eq!(provider.inner().storage_call_count(), 1);
    }
}
