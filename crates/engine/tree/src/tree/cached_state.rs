//! Cached state provider for block processing.
//!
//! Wraps a state provider with [`ExecutionCache`] to reduce database I/O during execution.

use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::sync::Arc;

// Re-export core cache types from `reth-execution-cache`.
pub(crate) use reth_execution_cache::{CacheStats, CachedStatus};
pub use reth_execution_cache::{
    CachedStateMetrics, ExecutionCache, PayloadExecutionCache, SavedCache,
};

/// A wrapper of a state provider and a shared cache.
///
/// The const generic `PREWARM` controls whether every cache miss is populated. This is only
/// relevant for pre-warm transaction execution with the intention to pre-populate the cache with
/// data for regular block execution. During regular block execution the cache doesn't need to be
/// populated because the actual EVM database [`State`](revm::database::State) also caches
/// internally during block execution and the cache is then updated after the block with the entire
/// [`BundleState`](reth_revm::db::BundleState) output of that block which contains all accessed
/// accounts, code, storage. See also [`ExecutionCache::insert_state`].
#[derive(Debug)]
pub struct CachedStateProvider<S, const PREWARM: bool = false> {
    /// The state provider
    state_provider: S,

    /// The caches used for the provider
    caches: ExecutionCache,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,

    /// Optional cache statistics for detailed block logging. Only tracked when slow block
    /// threshold is configured.
    cache_stats: Option<Arc<CacheStats>>,
}

impl<S> CachedStateProvider<S> {
    /// Creates a new [`CachedStateProvider`] from an [`ExecutionCache`], state provider, and
    /// [`CachedStateMetrics`].
    pub const fn new(
        state_provider: S,
        caches: ExecutionCache,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics, cache_stats: None }
    }
}

impl<S> CachedStateProvider<S, true> {
    /// Creates a new [`CachedStateProvider`] with prewarming enabled.
    pub const fn new_prewarm(
        state_provider: S,
        caches: ExecutionCache,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics, cache_stats: None }
    }
}

impl<S, const PREWARM: bool> CachedStateProvider<S, PREWARM> {
    /// Enables cache statistics tracking for detailed block logging.
    pub fn with_cache_stats(mut self, stats: Option<Arc<CacheStats>>) -> Self {
        self.cache_stats = stats;
        self
    }
}

impl<S: AccountReader, const PREWARM: bool> AccountReader for CachedStateProvider<S, PREWARM> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if PREWARM {
            match self.caches.get_or_try_insert_account_with(*address, || {
                self.state_provider.basic_account(address)
            })? {
                // During prewarm we only record stats (not prometheus metrics)
                CachedStatus::NotCached(value) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_account_miss();
                    }
                    Ok(value)
                }
                CachedStatus::Cached(value) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_account_hit();
                    }
                    Ok(value)
                }
            }
        } else if let Some(account) = self.caches.get_account(address) {
            self.metrics.increment_account_hits(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_account_hit();
            }
            Ok(account)
        } else {
            self.metrics.increment_account_misses(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_account_miss();
            }
            self.state_provider.basic_account(address)
        }
    }
}

impl<S: StateProvider, const PREWARM: bool> StateProvider for CachedStateProvider<S, PREWARM> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if PREWARM {
            match self.caches.get_or_try_insert_storage_with(account, storage_key, || {
                self.state_provider.storage(account, storage_key).map(Option::unwrap_or_default)
            })? {
                // During prewarm we only record stats (not prometheus metrics)
                CachedStatus::NotCached(value) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_storage_miss();
                    }
                    Ok(Some(value).filter(|v| !v.is_zero()))
                }
                CachedStatus::Cached(value) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_storage_hit();
                    }
                    Ok(Some(value).filter(|v| !v.is_zero()))
                }
            }
        } else if let Some(value) = self.caches.get_storage(&account, &storage_key) {
            self.metrics.increment_storage_hits(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_storage_hit();
            }
            Ok(Some(value).filter(|v| !v.is_zero()))
        } else {
            self.metrics.increment_storage_misses(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_storage_miss();
            }
            self.state_provider.storage(account, storage_key)
        }
    }
}

impl<S: BytecodeReader, const PREWARM: bool> BytecodeReader for CachedStateProvider<S, PREWARM> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if PREWARM {
            match self.caches.get_or_try_insert_code_with(*code_hash, || {
                self.state_provider.bytecode_by_hash(code_hash)
            })? {
                // During prewarm we only record stats (not prometheus metrics)
                CachedStatus::NotCached(code) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_code_miss();
                    }
                    Ok(code)
                }
                CachedStatus::Cached(code) => {
                    if let Some(stats) = &self.cache_stats {
                        stats.record_code_hit();
                    }
                    Ok(code)
                }
            }
        } else if let Some(code) = self.caches.get_code(code_hash) {
            self.metrics.increment_code_hits(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_code_hit();
            }
            Ok(code)
        } else {
            self.metrics.increment_code_misses(1.0);
            if let Some(stats) = &self.cache_stats {
                stats.record_code_miss();
            }
            self.state_provider.bytecode_by_hash(code_hash)
        }
    }
}

impl<S: StateRootProvider, const PREWARM: bool> StateRootProvider
    for CachedStateProvider<S, PREWARM>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.state_provider.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.state_provider.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_from_nodes_with_updates(input)
    }
}

impl<S: StateProofProvider, const PREWARM: bool> StateProofProvider
    for CachedStateProvider<S, PREWARM>
{
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.state_provider.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.state_provider.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        self.state_provider.witness(input, target)
    }
}

impl<S: StorageRootProvider, const PREWARM: bool> StorageRootProvider
    for CachedStateProvider<S, PREWARM>
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.state_provider.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.state_provider.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.state_provider.storage_multiproof(address, slots, hashed_storage)
    }
}

impl<S: BlockHashReader, const PREWARM: bool> BlockHashReader for CachedStateProvider<S, PREWARM> {
    fn block_hash(&self, number: alloy_primitives::BlockNumber) -> ProviderResult<Option<B256>> {
        self.state_provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: alloy_primitives::BlockNumber,
        end: alloy_primitives::BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.state_provider.canonical_hashes_range(start, end)
    }
}

impl<S: HashedPostStateProvider, const PREWARM: bool> HashedPostStateProvider
    for CachedStateProvider<S, PREWARM>
{
    fn hashed_post_state(&self, bundle_state: &reth_revm::db::BundleState) -> HashedPostState {
        self.state_provider.hashed_post_state(bundle_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    #[test]
    fn test_empty_storage_cached_state_provider() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let account = ExtendedAccount::new(0, U256::ZERO);

        let provider = MockEthProvider::default();
        provider.extend_accounts(vec![(address, account)]);

        let caches = ExecutionCache::new(1000);
        let state_provider =
            CachedStateProvider::new(provider, caches, CachedStateMetrics::zeroed());

        let res = state_provider.storage(address, storage_key);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), None);
    }

    #[test]
    fn test_uncached_storage_cached_state_provider() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);
        let account =
            ExtendedAccount::new(0, U256::ZERO).extend_storage(vec![(storage_key, storage_value)]);

        let provider = MockEthProvider::default();
        provider.extend_accounts(vec![(address, account)]);

        let caches = ExecutionCache::new(1000);
        let state_provider =
            CachedStateProvider::new(provider, caches, CachedStateMetrics::zeroed());

        let res = state_provider.storage(address, storage_key);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), Some(storage_value));
    }
}
