//! Execution cache module for ExecutionStage prewarming.
//!
//! Provides caching infrastructure to optimize state access during block execution
//! by maintaining in-memory copies of frequently accessed accounts, storage slots, and bytecode.

use alloy_primitives::{map::DefaultHashBuilder, Address, Bytes, StorageKey, StorageValue, B256};
use metrics::Gauge;
use mini_moka::sync::CacheBuilder;
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, Bytecode};
use reth_revm::db::BundleState;
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::{sync::Arc, time::Duration};

/// Type alias for the cache with custom hasher.
pub type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

/// A wrapper of a state provider and a shared cache.
#[derive(Debug)]
pub struct CachedStateProvider<S> {
    /// The state provider
    state_provider: S,

    /// The caches used for the provider
    caches: ExecutionCache,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,

    /// If prewarm enabled we populate every cache miss
    prewarm: bool,
}

impl<S> CachedStateProvider<S>
where
    S: StateProvider,
{
    /// Creates a new [`CachedStateProvider`] from an [`ExecutionCache`], state provider, and
    /// [`CachedStateMetrics`].
    pub const fn new(
        state_provider: S,
        caches: ExecutionCache,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics, prewarm: false }
    }
}

impl<S> CachedStateProvider<S> {
    /// Enables pre-warm mode so that every cache miss is populated.
    ///
    /// This is only relevant for pre-warm transaction execution with the intention to pre-populate
    /// the cache with data for regular block execution. During regular block execution the
    /// cache doesn't need to be populated because the actual EVM database
    /// [`State`](revm::database::State) also caches internally during block execution and the cache
    /// is then updated after the block with the entire [`BundleState`](reth_revm::db::BundleState)
    /// output of that block which contains all accessed accounts, code, storage.
    pub const fn prewarm(mut self) -> Self {
        self.prewarm = true;
        self
    }

    /// Returns whether this provider should pre-warm cache misses.
    const fn is_prewarm(&self) -> bool {
        self.prewarm
    }
}

/// Metrics for the cached state provider, showing hits / misses for each cache
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.stages.execution.caching")]
pub struct CachedStateMetrics {
    /// Code cache hits
    code_cache_hits: Gauge,

    /// Code cache misses
    code_cache_misses: Gauge,

    /// Code cache size
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
    pub fn reset(&self) {
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
    pub fn zeroed() -> Self {
        let zeroed = Self::default();
        zeroed.reset();
        zeroed
    }

    /// Updates the cache size metrics from the execution cache.
    pub fn update_cache_sizes(&self, cache: &ExecutionCache) {
        self.storage_cache_size.set(cache.total_storage_slots() as f64);
        self.account_cache_size.set(cache.account_cache.entry_count() as f64);
        self.code_cache_size.set(cache.code_cache.entry_count() as f64);
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

        if self.is_prewarm() {
            self.caches.account_cache.insert(*address, res);
        }
        Ok(res)
    }
}

/// Represents the status of a storage slot in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotStatus {
    /// The account's storage cache doesn't exist.
    NotCached,
    /// The storage slot exists in cache and is empty (value is zero).
    Empty,
    /// The storage slot exists in cache and has a specific non-zero value.
    Value(StorageValue),
}

impl<S: StateProvider> StateProvider for CachedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.caches.get_storage(&account, &storage_key) {
            (SlotStatus::NotCached, maybe_cache) => {
                let final_res = self.state_provider.storage(account, storage_key)?;

                if self.is_prewarm() {
                    let account_cache = maybe_cache.unwrap_or_default();
                    account_cache.insert_storage(storage_key, final_res);
                    // we always need to insert the value to update the weights.
                    // Note: there exists a race when the storage cache did not exist yet and two
                    // consumers looking up a storage value for this account for the first time,
                    // however we can assume that this will only happen for the very first
                    // (most likely the same) value, and don't expect that this
                    // will accidentally replace an account storage cache with
                    // additional values.
                    self.caches.insert_storage_cache(account, account_cache);
                }

                self.metrics.storage_cache_misses.increment(1);
                Ok(final_res)
            }
            (SlotStatus::Empty, _) => {
                self.metrics.storage_cache_hits.increment(1);
                Ok(None)
            }
            (SlotStatus::Value(value), _) => {
                self.metrics.storage_cache_hits.increment(1);
                Ok(Some(value))
            }
        }
    }
}

impl<S: BytecodeReader> BytecodeReader for CachedStateProvider<S> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(res) = self.caches.code_cache.get(code_hash) {
            self.metrics.code_cache_hits.increment(1);
            return Ok(res)
        }

        self.metrics.code_cache_misses.increment(1);

        let final_res = self.state_provider.bytecode_by_hash(code_hash)?;

        if self.is_prewarm() {
            self.caches.code_cache.insert(*code_hash, final_res.clone());
        }

        Ok(final_res)
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

impl<S: StateRootProvider> StateRootProvider for CachedStateProvider<S> {
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

    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
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

impl<S: HashedPostStateProvider> HashedPostStateProvider for CachedStateProvider<S> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        self.state_provider.hashed_post_state(bundle_state)
    }
}

/// Execution cache used during block processing.
///
/// Optimizes state access by maintaining in-memory copies of frequently accessed
/// accounts, storage slots, and bytecode. Works in conjunction with prewarming
/// to reduce database I/O during block execution.
#[derive(Debug, Clone)]
pub struct ExecutionCache {
    /// Cache for contract bytecode, keyed by code hash.
    code_cache: Cache<B256, Option<Bytecode>>,

    /// Per-account storage cache: outer cache keyed by Address, inner cache tracks that account's
    /// storage slots.
    storage_cache: Cache<Address, Arc<AccountStorageCache>>,

    /// Cache for basic account information (nonce, balance, code hash).
    account_cache: Cache<Address, Option<Account>>,
}

impl ExecutionCache {
    /// Get storage value from hierarchical cache.
    ///
    /// Returns a tuple of:
    /// - `SlotStatus` indicating whether:
    ///   - `NotCached`: The account's storage cache doesn't exist
    ///   - `Empty`: The slot exists in the account's cache but is empty
    ///   - `Value`: The slot exists and has a specific value
    /// - `Option<Arc<AccountStorageCache>>`: The account's storage cache if it exists
    pub fn get_storage(
        &self,
        address: &Address,
        key: &StorageKey,
    ) -> (SlotStatus, Option<Arc<AccountStorageCache>>) {
        match self.storage_cache.get(address) {
            None => (SlotStatus::NotCached, None),
            Some(account_cache) => {
                let status = account_cache.get_storage(key);
                (status, Some(account_cache))
            }
        }
    }

    /// Insert storage value into hierarchical cache
    #[cfg(test)]
    pub fn insert_storage(&self, address: Address, key: StorageKey, value: Option<StorageValue>) {
        self.insert_storage_bulk(address, [(key, value)]);
    }

    /// Insert multiple storage values into hierarchical cache for a single account
    ///
    /// This method is optimized for inserting multiple storage values for the same address
    /// by doing the account cache lookup only once instead of for each key-value pair.
    pub fn insert_storage_bulk<I>(&self, address: Address, storage_entries: I)
    where
        I: IntoIterator<Item = (StorageKey, Option<StorageValue>)>,
    {
        let account_cache = self.storage_cache.get(&address).unwrap_or_default();

        for (key, value) in storage_entries {
            account_cache.insert_storage(key, value);
        }

        // Insert to the cache so that moka picks up on the changed size, even though the actual
        // value (the Arc<AccountStorageCache>) is the same
        self.storage_cache.insert(address, account_cache);
    }

    /// Inserts the [`AccountStorageCache`].
    pub fn insert_storage_cache(&self, address: Address, storage_cache: Arc<AccountStorageCache>) {
        self.storage_cache.insert(address, storage_cache);
    }

    /// Invalidate storage for specific account
    pub fn invalidate_account_storage(&self, address: &Address) {
        self.storage_cache.invalidate(address);
    }

    /// Returns the total number of storage slots cached across all accounts
    pub fn total_storage_slots(&self) -> usize {
        self.storage_cache.iter().map(|addr| addr.len()).sum()
    }
}

/// A builder for [`ExecutionCache`].
#[derive(Debug)]
pub struct ExecutionCacheBuilder {
    /// Code cache entries
    code_cache_entries: u64,

    /// Storage cache entries
    storage_cache_entries: u64,

    /// Account cache entries
    account_cache_entries: u64,
}

impl ExecutionCacheBuilder {
    /// Build an [`ExecutionCache`] struct, so that execution caches can be easily cloned.
    pub fn build_caches(self, total_cache_size: u64) -> ExecutionCache {
        let storage_cache_size = (total_cache_size * 8888) / 10000; // 88.88% of total
        let account_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total
        let code_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total

        const EXPIRY_TIME: Duration = Duration::from_secs(7200); // 2 hours
        const TIME_TO_IDLE: Duration = Duration::from_secs(3600); // 1 hour

        let storage_cache = CacheBuilder::new(self.storage_cache_entries)
            .weigher(|_key: &Address, value: &Arc<AccountStorageCache>| -> u32 {
                // values based on results from measure_storage_cache_overhead test
                let base_weight = 39_000;
                let slots_weight = value.len() * 218;
                (base_weight + slots_weight) as u32
            })
            .max_capacity(storage_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(DefaultHashBuilder::default());

        let account_cache = CacheBuilder::new(self.account_cache_entries)
            .weigher(|_key: &Address, value: &Option<Account>| -> u32 {
                // Account has a fixed size (none, balance, code_hash)
                20 + std::mem::size_of_val(value) as u32
            })
            .max_capacity(account_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(DefaultHashBuilder::default());

        let code_cache = CacheBuilder::new(self.code_cache_entries)
            .weigher(|_key: &B256, value: &Option<Bytecode>| -> u32 {
                let code_size = match value {
                    Some(bytecode) => {
                        // base weight + actual (padded) bytecode size + size of the jump table
                        (std::mem::size_of_val(value) +
                            bytecode.bytecode().len() +
                            bytecode
                                .legacy_jump_table()
                                .map(|table| table.as_slice().len())
                                .unwrap_or_default()) as u32
                    }
                    None => std::mem::size_of_val(value) as u32,
                };
                32 + code_size
            })
            .max_capacity(code_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(DefaultHashBuilder::default());

        ExecutionCache { code_cache, storage_cache, account_cache }
    }
}

impl Default for ExecutionCacheBuilder {
    fn default() -> Self {
        // With weigher and max_capacity in place, these numbers represent
        // the maximum number of entries that can be stored, not the actual
        // memory usage which is controlled by max_capacity.
        //
        // Code cache: up to 10M entries but limited to 0.5GB
        // Storage cache: up to 10M accounts but limited to 8GB
        // Account cache: up to 10M accounts but limited to 0.5GB
        Self {
            code_cache_entries: 10_000_000,
            storage_cache_entries: 10_000_000,
            account_cache_entries: 10_000_000,
        }
    }
}

/// Cache for an individual account's storage slots.
///
/// This represents the second level of the hierarchical storage cache.
/// Each account gets its own `AccountStorageCache` to store accessed storage slots.
#[derive(Debug, Clone)]
pub struct AccountStorageCache {
    /// Map of storage keys to their cached values.
    slots: Cache<StorageKey, Option<StorageValue>>,
}

impl AccountStorageCache {
    /// Create a new [`AccountStorageCache`]
    pub fn new(max_slots: u64) -> Self {
        Self {
            slots: CacheBuilder::new(max_slots).build_with_hasher(DefaultHashBuilder::default()),
        }
    }

    /// Get a storage value from this account's cache.
    /// - `NotCached`: The slot is not in the cache
    /// - `Empty`: The slot is empty
    /// - `Value`: The slot has a specific value
    pub fn get_storage(&self, key: &StorageKey) -> SlotStatus {
        match self.slots.get(key) {
            None => SlotStatus::NotCached,
            Some(None) => SlotStatus::Empty,
            Some(Some(value)) => SlotStatus::Value(value),
        }
    }

    /// Insert a storage value
    pub fn insert_storage(&self, key: StorageKey, value: Option<StorageValue>) {
        self.slots.insert(key, value);
    }

    /// Returns the number of slots in the cache
    pub fn len(&self) -> usize {
        self.slots.entry_count() as usize
    }

    /// Returns true if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for AccountStorageCache {
    fn default() -> Self {
        // With weigher and max_capacity in place, this number represents
        // the maximum number of entries that can be stored, not the actual
        // memory usage which is controlled by storage cache's max_capacity.
        Self::new(1_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn test_get_storage_populated() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, Some(storage_value));

        let (slot_status, _) = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::Value(storage_value));
    }

    #[test]
    fn test_get_storage_not_cached() {
        let storage_key = StorageKey::random();
        let address = Address::random();

        let caches = ExecutionCacheBuilder::default().build_caches(1000);

        let (slot_status, _) = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::NotCached);
    }

    #[test]
    fn test_get_storage_empty() {
        let address = Address::random();
        let storage_key = StorageKey::random();

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, None);

        let (slot_status, _) = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::Empty);
    }

    #[test]
    fn test_account_storage_cache_default() {
        let cache = AccountStorageCache::default();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_metrics_reset() {
        let metrics = CachedStateMetrics::zeroed();
        metrics.code_cache_hits.increment(5);
        metrics.reset();
        // After reset, the counter should be 0
    }
}
