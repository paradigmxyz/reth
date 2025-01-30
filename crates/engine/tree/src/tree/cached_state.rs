//! Implements a state provider that has a shared cache in front of it.
use alloy_primitives::{map::B256HashMap, Address, StorageKey, StorageValue, B256};
use metrics::Gauge;
use mini_moka::sync::CacheBuilder;
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProofProvider, StateProvider,
    StateRootProvider, StorageRootProvider,
};
use reth_revm::db::BundleState;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use revm_primitives::map::DefaultHashBuilder;
use std::time::Instant;

type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

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

impl<S> CachedStateProvider<S> {
    /// Creates a new [`SavedCache`] from the given state updates, block hash, and account storage
    /// roots.
    ///
    /// This does not update the code cache, because no changes are required to the code cache on
    /// state change.
    ///
    /// NOTE: Consumers should ensure that these caches are not in use by a state provider for a
    /// previous block - otherwise, this update will cause that state provider to contain future
    /// state, which would be incorrect.
    pub(crate) fn save_cache(
        self,
        executed_block_hash: B256,
        state_updates: &BundleState,
    ) -> Result<SavedCache, ()> {
        let Self { caches, metrics, state_provider: _ } = self;
        let start = Instant::now();

        for (addr, account) in &state_updates.state {
            // If the account was not modified, as in not changed and not destroyed, then we have
            // nothing to do w.r.t. this particular account and can move on
            if account.status.is_not_modified() {
                continue
            }

            // if the account was destroyed, invalidate from the account / storage caches
            if account.was_destroyed() {
                // invalidate the account cache entry if destroyed
                caches.account_cache.invalidate(addr);

                caches.invalidate_account_storage(addr);
                continue
            }

            // if we have an account that was modified, but it has a `None` account info, some wild
            // error has occurred because this state should be unrepresentable. An account with
            // `None` current info, should be destroyed.
            let Some(ref account_info) = account.info else {
                todo!("error handling - a modified account has None info")
            };

            // insert will update if present, so we just use the new account info as the new value
            // for the account cache
            caches.account_cache.insert(*addr, Some(Account::from(account_info)));

            // now we iterate over all storage and make updates to the cached storage values
            for (storage_key, slot) in &account.storage {
                // we convert the storage key from U256 to B256 because that is how it's represented
                // in the cache
                caches.insert_storage(*addr, (*storage_key).into(), Some(slot.present_value));
            }
        }

        // set metrics
        metrics.storage_cache_size.set(caches.total_storage_slots() as f64);
        metrics.account_cache_size.set(caches.account_cache.entry_count() as f64);
        metrics.code_cache_size.set(caches.code_cache.entry_count() as f64);

        tracing::debug!(target: "engine::caching", update_latency=?start.elapsed(), "Updated state caches");

        // create a saved cache with the executed block hash, same metrics, and updated caches
        let saved_cache = SavedCache { hash: executed_block_hash, caches, metrics };

        Ok(saved_cache)
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

    /// Storage cache size
    ///
    /// NOTE: this uses the moka caches' `entry_count`, NOT the `weighted_size` method to calculate
    /// size.
    code_cache_size: Gauge,

    /// Storage cache hits
    storage_cache_hits: Gauge,

    /// Storage cache misses
    storage_cache_misses: Gauge,

    /// Storage cache size
    ///
    /// NOTE: this uses the moka caches' `entry_count`, NOT the `weighted_size` method to calculate
    /// size.
    storage_cache_size: Gauge,

    /// Account cache hits
    account_cache_hits: Gauge,

    /// Account cache misses
    account_cache_misses: Gauge,

    /// Account cache size
    ///
    /// NOTE: this uses the moka caches' `entry_count`, NOT the `weighted_size` method to calculate
    /// size.
    account_cache_size: Gauge,
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
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if let Some(res) = self.caches.account_cache.get(address) {
            self.metrics.account_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.account_cache_misses.increment(1);

        let res = self.state_provider.basic_account(address)?;
        self.caches.account_cache.insert(*address, res);
        Ok(res)
    }
}

impl<S: StateProvider> StateProvider for CachedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if let Some(res) = self.caches.get_storage(&account, &storage_key) {
            self.metrics.storage_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.storage_cache_misses.increment(1);

        let final_res = self.state_provider.storage(account, storage_key)?;
        self.caches.insert_storage(account, storage_key, final_res);
        Ok(final_res)
    }

    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(res) = self.caches.code_cache.get(code_hash) {
            self.metrics.code_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.code_cache_misses.increment(1);

        let final_res = self.state_provider.bytecode_by_hash(code_hash)?;
        self.caches.code_cache.insert(*code_hash, final_res.clone());
        Ok(final_res)
    }
}

impl<S: StateRootProvider> StateRootProvider for CachedStateProvider<S> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.state_provider.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.state_provider.state_root_from_nodes(input)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_from_nodes_with_updates(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_with_updates(hashed_state)
    }
}

impl<S: StateProofProvider> StateProofProvider for CachedStateProvider<S> {
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
    ) -> ProviderResult<B256HashMap<alloy_primitives::Bytes>> {
        self.state_provider.witness(input, target)
    }
}

impl<S: StorageRootProvider> StorageRootProvider for CachedStateProvider<S> {
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

impl<S: HashedPostStateProvider> HashedPostStateProvider for CachedStateProvider<S> {
    fn hashed_post_state(&self, bundle_state: &reth_revm::db::BundleState) -> HashedPostState {
        self.state_provider.hashed_post_state(bundle_state)
    }
}

/// The set of caches that are used in the [`CachedStateProvider`].
#[derive(Debug, Clone)]
pub(crate) struct ProviderCaches {
    /// The cache for bytecode
    code_cache: Cache<B256, Option<Bytecode>>,

    /// The cache for storage, organized hierarchically by account
    storage_cache: Cache<Address, AccountStorageCache>,

    /// The cache for basic accounts
    account_cache: Cache<Address, Option<Account>>,
}

impl ProviderCaches {
    /// Get storage value from hierarchical cache
    pub(crate) fn get_storage(
        &self,
        address: &Address,
        key: &StorageKey,
    ) -> Option<Option<StorageValue>> {
        self.storage_cache.get(address).and_then(|account_cache| account_cache.get_storage(key))
    }

    /// Insert storage value into hierarchical cache
    pub(crate) fn insert_storage(
        &self,
        address: Address,
        key: StorageKey,
        value: Option<StorageValue>,
    ) {
        let account_cache = self.storage_cache.get(&address).unwrap_or_default();

        account_cache.insert_storage(key, value);

        self.storage_cache.insert(address, account_cache);
    }

    /// Invalidate storage for specific account
    pub(crate) fn invalidate_account_storage(&self, address: &Address) {
        self.storage_cache.invalidate(address);
    }

    /// Returns the total number of storage slots cached across all accounts
    pub(crate) fn total_storage_slots(&self) -> usize {
        self.storage_cache.iter().map(|addr| addr.len()).sum()
    }
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
            code_cache: CacheBuilder::new(self.code_cache_size)
                .build_with_hasher(DefaultHashBuilder::default()),
            storage_cache: CacheBuilder::new(self.storage_cache_size)
                .build_with_hasher(DefaultHashBuilder::default()),
            account_cache: CacheBuilder::new(self.account_cache_size)
                .build_with_hasher(DefaultHashBuilder::default()),
        }
    }
}

impl Default for ProviderCacheBuilder {
    fn default() -> Self {
        // moka caches have been benchmarked up to 800k entries, so we just use 10M on account and
        // code cache, and 100M for storage cache, optimizing for hitrate over memory consumption.
        //
        // Heuristic for storage cache memory consumption:
        // * the keys are addr (20 bytes) + storage key (32 bytes)
        // * the values are 32 bytes
        // So this means roughly 84 bytes per entry.
        //
        // 100M * 84 = 8.4 GB
        //
        // Code cache can be much much larger - this needs to be space instead of element bound.
        // Accounts can be element bound but memory calculation TODO
        //
        // See: https://github.com/moka-rs/moka/wiki#admission-and-eviction-policies
        Self {
            code_cache_size: 1_000_000,
            storage_cache_size: 10_000_000,
            account_cache_size: 10_000_000,
        }
    }
}

/// A saved cache that has been used for executing a specific block, which has been updated for its
/// execution.
#[derive(Debug)]
pub(crate) struct SavedCache {
    /// The hash of the block these caches were used to execute.
    hash: B256,

    /// The caches used for the provider.
    caches: ProviderCaches,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,
}

impl SavedCache {
    /// Returns the hash for this cache
    pub(crate) const fn executed_block_hash(&self) -> B256 {
        self.hash
    }

    /// Splits the cache into its caches and metrics, consuming it.
    pub(crate) fn split(self) -> (ProviderCaches, CachedStateMetrics) {
        (self.caches, self.metrics)
    }
}

/// Cache for an account's storage slots
#[derive(Debug, Clone)]
pub(crate) struct AccountStorageCache {
    /// The storage slots for this account
    slots: Cache<StorageKey, Option<StorageValue>>,
}

impl AccountStorageCache {
    /// Create a new [`AccountStorageCache`]
    pub(crate) fn new(max_slots: u64) -> Self {
        Self {
            slots: CacheBuilder::new(max_slots).build_with_hasher(DefaultHashBuilder::default()),
        }
    }

    /// Get a storage value
    pub(crate) fn get_storage(&self, key: &StorageKey) -> Option<Option<StorageValue>> {
        self.slots.get(key)
    }

    /// Insert a storage value
    pub(crate) fn insert_storage(&self, key: StorageKey, value: Option<StorageValue>) {
        self.slots.insert(key, value);
    }

    /// Returns the number of slots in the cache
    pub(crate) fn len(&self) -> usize {
        self.slots.entry_count() as usize
    }
}

impl Default for AccountStorageCache {
    fn default() -> Self {
        Self::new(10000)
    }
}
