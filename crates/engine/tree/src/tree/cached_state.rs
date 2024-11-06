//! Implements a state provider that has a shared cache in front of it.
#![allow(dead_code)]
use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use metrics::Gauge;
use moka::sync::Cache;
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateRootProvider,
    StorageRootProvider,
};

/// A wrapper of a state provider and a shared cache.
pub(crate) struct CachedStateProvider<S> {
    /// The state provider
    state_provider: S,

    /// The caches used for the provider
    caches: ProviderCaches,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,
}

impl<S> CachedStateProvider<S>
where
    S: StateProvider,
{
    /// Creates a new [`CachedStateProvider`] that contains the given state provider and caches.
    pub(crate) const fn new(
        state_provider: S,
        code_cache: Cache<B256, Option<Bytecode>>,
        storage_cache: Cache<(Address, StorageKey), Option<StorageValue>>,
        account_cache: Cache<Address, Option<Account>>,
        metrics: CachedStateMetrics,
    ) -> Self {
        let caches = ProviderCaches { code_cache, account_cache, storage_cache };

        Self { state_provider, caches, metrics }
    }

    /// Creates a new [`CachedStateProvider`] from a [`ProviderCaches`], state provider, and
    /// [`CachedStateMetrics`].
    pub(crate) const fn new_with_caches(
        state_provider: S,
        caches: ProviderCaches,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics }
    }
}

/// Metrics for the cached state provider, showing hits / misses for each cache
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub(crate) struct CachedStateMetrics {
    /// Code cache hits
    code_cache_hits: Gauge,

    /// Code cache misses
    code_cache_misses: Gauge,

    /// Storage cache hits
    storage_cache_hits: Gauge,

    /// Storage cache misses
    storage_cache_misses: Gauge,

    /// Account cache hits
    account_cache_hits: Gauge,

    /// Account cache misses
    account_cache_misses: Gauge,
}

impl CachedStateMetrics {
    /// Sets all values to zero, indicating that a new block is being executed.
    pub(crate) fn reset(&self) {
        // code cache
        self.code_cache_hits.set(0);
        self.code_cache_misses.set(0);

        // storage cache
        self.storage_cache_hits.set(0);
        self.storage_cache_misses.set(0);

        // account cache
        self.account_cache_hits.set(0);
        self.account_cache_misses.set(0);
    }

    /// Returns a new zeroed-out instance of [`CachedStateMetrics`].
    pub(crate) fn zeroed() -> Self {
        let zeroed = Self::default();
        zeroed.reset();
        zeroed
    }
}

impl<S: AccountReader> AccountReader for CachedStateProvider<S> {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        if let Some(res) = self.caches.account_cache.get(&address) {
            self.metrics.account_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.account_cache_misses.increment(1);

        let res = self.state_provider.basic_account(address)?;
        self.caches.account_cache.insert(address, res);
        Ok(res)
    }
}

impl<S: StateProvider> StateProvider for CachedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if let Some(res) = self.caches.storage_cache.get(&(account, storage_key)) {
            self.metrics.storage_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.storage_cache_misses.increment(1);

        let final_res = self.state_provider.storage(account, storage_key)?;
        self.caches.storage_cache.insert((account, storage_key), final_res);
        Ok(final_res)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(res) = self.caches.code_cache.get(&code_hash) {
            self.metrics.code_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.code_cache_misses.increment(1);

        let final_res = self.state_provider.bytecode_by_hash(code_hash)?;
        self.caches.code_cache.insert(code_hash, final_res.clone());
        Ok(final_res)
    }
}

impl<S: StateRootProvider> StateRootProvider for CachedStateProvider<S> {
    fn state_root(&self, hashed_state: reth_trie::HashedPostState) -> ProviderResult<B256> {
        self.state_provider.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: reth_trie::TrieInput) -> ProviderResult<B256> {
        self.state_provider.state_root_from_nodes(input)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: reth_trie::TrieInput,
    ) -> ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.state_provider.state_root_from_nodes_with_updates(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: reth_trie::HashedPostState,
    ) -> ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.state_provider.state_root_with_updates(hashed_state)
    }
}

impl<S: StateProofProvider> StateProofProvider for CachedStateProvider<S> {
    fn proof(
        &self,
        input: reth_trie::TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<reth_trie::AccountProof> {
        self.state_provider.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: reth_trie::TrieInput,
        targets: alloy_primitives::map::HashMap<B256, alloy_primitives::map::HashSet<B256>>,
    ) -> ProviderResult<reth_trie::MultiProof> {
        self.state_provider.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: reth_trie::TrieInput,
        target: reth_trie::HashedPostState,
    ) -> ProviderResult<alloy_primitives::map::HashMap<B256, alloy_primitives::Bytes>> {
        self.state_provider.witness(input, target)
    }
}

impl<S: StorageRootProvider> StorageRootProvider for CachedStateProvider<S> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: reth_trie::HashedStorage,
    ) -> ProviderResult<B256> {
        self.state_provider.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: reth_trie::HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        self.state_provider.storage_proof(address, slot, hashed_storage)
    }
}

impl<S: BlockHashReader> BlockHashReader for CachedStateProvider<S> {
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

/// The set of caches that are used in the [`CachedStateProvider`].
#[derive(Debug, Clone)]
pub(crate) struct ProviderCaches {
    /// The cache for bytecode
    code_cache: Cache<B256, Option<Bytecode>>,

    /// The cache for storage
    storage_cache: Cache<(Address, StorageKey), Option<StorageValue>>,

    /// The cache for basic accounts
    account_cache: Cache<Address, Option<Account>>,
}

/// A builder for [`ProviderCaches`].
#[derive(Debug)]
pub(crate) struct ProviderCacheBuilder {
    /// Code cache size
    code_cache_size: u64,

    /// Storage cache size
    storage_cache_size: u64,

    /// Account cache size
    account_cache_size: u64,
}

impl ProviderCacheBuilder {
    /// Build a [`ProviderCaches`] struct, so that provider caches can be easily cloned.
    pub(crate) fn build_caches(self) -> ProviderCaches {
        ProviderCaches {
            code_cache: Cache::new(self.code_cache_size),
            storage_cache: Cache::new(self.storage_cache_size),
            account_cache: Cache::new(self.account_cache_size),
        }
    }
}

impl Default for ProviderCacheBuilder {
    fn default() -> Self {
        // moka caches have been benchmarked up to 800k entries, so we just use 1M, optimizing for
        // hitrate over memory consumption.
        //
        // See: https://github.com/moka-rs/moka/wiki#admission-and-eviction-policies
        Self { code_cache_size: 1000000, storage_cache_size: 1000000, account_cache_size: 1000000 }
    }
}
