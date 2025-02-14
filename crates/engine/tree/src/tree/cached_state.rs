//! Implements a state provider that has a shared cache in front of it.
use alloy_primitives::{map::B256Map, Address, StorageKey, StorageValue, B256};
use metrics::Gauge;
use mini_moka::sync::CacheBuilder;
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, Bytecode};
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
use std::time::{Duration, Instant};
use tracing::{debug, trace};

pub(crate) type Cache<K, V> =
    mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

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
    /// Creates a new [`SavedCache`] from the given state updates and executed block hash.
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
                trace!(target: "engine::caching", ?account, "Account with None account info found in state updates");
                return Err(())
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

        debug!(target: "engine::caching", update_latency=?start.elapsed(), "Updated state caches");

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
    ) -> ProviderResult<B256Map<alloy_primitives::Bytes>> {
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
    /// Code cache entries
    code_cache_entries: u64,

    /// Storage cache entries
    storage_cache_entries: u64,

    /// Account cache entries
    account_cache_entries: u64,
}

impl ProviderCacheBuilder {
    /// Build a [`ProviderCaches`] struct, so that provider caches can be easily cloned.
    pub(crate) fn build_caches(self, total_cache_size: u64) -> ProviderCaches {
        let storage_cache_size = (total_cache_size * 8888) / 10000; // 88.88% of total
        let account_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total
        let code_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total

        const EXPIRY_TIME: Duration = Duration::from_secs(7200); // 2 hours
        const TIME_TO_IDLE: Duration = Duration::from_secs(3600); // 1 hour

        let storage_cache = CacheBuilder::new(self.storage_cache_entries)
            .weigher(|_key: &Address, value: &AccountStorageCache| -> u32 {
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
                match value {
                    Some(account) => {
                        let mut weight = 40;
                        if account.nonce != 0 {
                            weight += 32;
                        }
                        if !account.balance.is_zero() {
                            weight += 32;
                        }
                        if account.bytecode_hash.is_some() {
                            weight += 33; // size of Option<B256>
                        } else {
                            weight += 8; // size of None variant
                        }
                        weight as u32
                    }
                    None => 8, // size of None variant
                }
            })
            .max_capacity(account_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(DefaultHashBuilder::default());

        let code_cache = CacheBuilder::new(self.code_cache_entries)
            .weigher(|_key: &B256, value: &Option<Bytecode>| -> u32 {
                match value {
                    Some(bytecode) => {
                        // base weight + actual bytecode size
                        (40 + bytecode.len()) as u32
                    }
                    None => 8, // size of None variant
                }
            })
            .max_capacity(code_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(DefaultHashBuilder::default());

        ProviderCaches { code_cache, storage_cache, account_cache }
    }
}

impl Default for ProviderCacheBuilder {
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
        // With weigher and max_capacity in place, this number represents
        // the maximum number of entries that can be stored, not the actual
        // memory usage which is controlled by storage cache's max_capacity.
        Self::new(1_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use std::mem::size_of;

    mod tracking_allocator {
        use std::{
            alloc::{GlobalAlloc, Layout, System},
            sync::atomic::{AtomicUsize, Ordering},
        };

        #[derive(Debug)]
        pub(crate) struct TrackingAllocator {
            allocated: AtomicUsize,
            total_allocated: AtomicUsize,
            inner: System,
        }

        impl TrackingAllocator {
            pub(crate) const fn new() -> Self {
                Self {
                    allocated: AtomicUsize::new(0),
                    total_allocated: AtomicUsize::new(0),
                    inner: System,
                }
            }

            pub(crate) fn reset(&self) {
                self.allocated.store(0, Ordering::SeqCst);
                self.total_allocated.store(0, Ordering::SeqCst);
            }

            pub(crate) fn total_allocated(&self) -> usize {
                self.total_allocated.load(Ordering::SeqCst)
            }
        }

        unsafe impl GlobalAlloc for TrackingAllocator {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let ret = self.inner.alloc(layout);
                if !ret.is_null() {
                    self.allocated.fetch_add(layout.size(), Ordering::SeqCst);
                    self.total_allocated.fetch_add(layout.size(), Ordering::SeqCst);
                }
                ret
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                self.allocated.fetch_sub(layout.size(), Ordering::SeqCst);
                self.inner.dealloc(ptr, layout)
            }
        }
    }

    use tracking_allocator::TrackingAllocator;

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

    fn measure_allocation<T, F>(f: F) -> (usize, T)
    where
        F: FnOnce() -> T,
    {
        ALLOCATOR.reset();
        let result = f();
        let total = ALLOCATOR.total_allocated();
        (total, result)
    }

    #[test]
    fn measure_storage_cache_overhead() {
        let (base_overhead, cache) = measure_allocation(|| AccountStorageCache::new(1000));
        println!("Base AccountStorageCache overhead: {} bytes", base_overhead);
        let mut rng = rand::thread_rng();

        let key = StorageKey::random();
        let value = StorageValue::from(rng.gen::<u128>());
        let (first_slot, _) = measure_allocation(|| {
            cache.insert_storage(key, Some(value));
        });
        println!("First slot insertion overhead: {} bytes", first_slot);

        const TOTAL_SLOTS: usize = 10_000;
        let (test_slots, _) = measure_allocation(|| {
            for _ in 0..TOTAL_SLOTS {
                let key = StorageKey::random();
                let value = StorageValue::from(rng.gen::<u128>());
                cache.insert_storage(key, Some(value));
            }
        });
        println!("Average overhead over {} slots: {} bytes", TOTAL_SLOTS, test_slots / TOTAL_SLOTS);

        println!("\nTheoretical sizes:");
        println!("StorageKey size: {} bytes", size_of::<StorageKey>());
        println!("StorageValue size: {} bytes", size_of::<StorageValue>());
        println!("Option<StorageValue> size: {} bytes", size_of::<Option<StorageValue>>());
        println!("Option<B256> size: {} bytes", size_of::<Option<B256>>());
    }
}
