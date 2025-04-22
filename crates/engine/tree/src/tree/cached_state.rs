//! Implements a state provider that has a shared cache in front of it.
use alloy_primitives::{Address, StorageKey, StorageValue, B256};
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
use std::time::Duration;
use tracing::trace;

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

/// Represents the status of a storage slot in the cache
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlotStatus {
    /// The account's storage cache doesn't exist
    NotCached,
    /// The storage slot is empty (either not in cache or explicitly None)
    Empty,
    /// The storage slot has a value
    Value(StorageValue),
}

impl<S: StateProvider> StateProvider for CachedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.caches.get_storage(&account, &storage_key) {
            SlotStatus::NotCached => {
                self.metrics.storage_cache_misses.increment(1);
                let final_res = self.state_provider.storage(account, storage_key)?;
                self.caches.insert_storage(account, storage_key, final_res);
                Ok(final_res)
            }
            SlotStatus::Empty => {
                self.metrics.storage_cache_hits.increment(1);
                Ok(None)
            }
            SlotStatus::Value(value) => {
                self.metrics.storage_cache_hits.increment(1);
                Ok(Some(value))
            }
        }
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

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
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
    /// Get storage value from hierarchical cache.
    ///
    /// Returns a `SlotStatus` indicating whether:
    /// - `NotCached`: The account's storage cache doesn't exist
    /// - `Empty`: The slot exists in the account's cache but is empty
    /// - `Value`: The slot exists and has a specific value
    pub(crate) fn get_storage(&self, address: &Address, key: &StorageKey) -> SlotStatus {
        match self.storage_cache.get(address) {
            None => SlotStatus::NotCached,
            Some(account_cache) => account_cache.get_storage(key),
        }
    }

    /// Insert storage value into hierarchical cache
    pub(crate) fn insert_storage(
        &self,
        address: Address,
        key: StorageKey,
        value: Option<StorageValue>,
    ) {
        let account_cache = self.storage_cache.get(&address).unwrap_or_else(|| {
            let account_cache = AccountStorageCache::default();
            self.storage_cache.insert(address, account_cache.clone());
            account_cache
        });
        account_cache.insert_storage(key, value);
    }

    /// Invalidate storage for specific account
    pub(crate) fn invalidate_account_storage(&self, address: &Address) {
        self.storage_cache.invalidate(address);
    }

    /// Returns the total number of storage slots cached across all accounts
    pub(crate) fn total_storage_slots(&self) -> usize {
        self.storage_cache.iter().map(|addr| addr.len()).sum()
    }

    /// Inserts the [`BundleState`] entries into the cache.
    ///
    /// Entries are inserted in the following order:
    /// 1. Bytecodes
    /// 2. Storage slots
    /// 3. Accounts
    ///
    /// The order is important, because the access patterns are Account -> Bytecode and Account ->
    /// Storage slot. If we update the account first, it may point to a code hash that doesn't have
    /// the associated bytecode anywhere yet.
    ///
    /// Returns an error if the state can't be cached and should be discarded.
    pub(crate) fn insert_state(&self, state_updates: &BundleState) -> Result<(), ()> {
        // Insert bytecodes
        for (code_hash, bytecode) in &state_updates.contracts {
            self.code_cache.insert(*code_hash, Some(Bytecode(bytecode.clone())));
        }

        for (addr, account) in &state_updates.state {
            // If the account was not modified, as in not changed and not destroyed, then we have
            // nothing to do w.r.t. this particular account and can move on
            if account.status.is_not_modified() {
                continue
            }

            // If the account was destroyed, invalidate from the account / storage caches
            if account.was_destroyed() {
                // Invalidate the account cache entry if destroyed
                self.account_cache.invalidate(addr);

                self.invalidate_account_storage(addr);
                continue
            }

            // If we have an account that was modified, but it has a `None` account info, some wild
            // error has occurred because this state should be unrepresentable. An account with
            // `None` current info, should be destroyed.
            let Some(ref account_info) = account.info else {
                trace!(target: "engine::caching", ?account, "Account with None account info found in state updates");
                return Err(())
            };

            // Now we iterate over all storage and make updates to the cached storage values
            for (storage_key, slot) in &account.storage {
                // We convert the storage key from U256 to B256 because that is how it's represented
                // in the cache
                self.insert_storage(*addr, (*storage_key).into(), Some(slot.present_value));
            }

            // Insert will update if present, so we just use the new account info as the new value
            // for the account cache
            self.account_cache.insert(*addr, Some(Account::from(account_info)));
        }

        Ok(())
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
#[derive(Debug, Clone)]
pub(crate) struct SavedCache {
    /// The hash of the block these caches were used to execute.
    hash: B256,

    /// The caches used for the provider.
    caches: ProviderCaches,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,
}

impl SavedCache {
    /// Creates a new instance with the internals
    pub(super) const fn new(
        hash: B256,
        caches: ProviderCaches,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { hash, caches, metrics }
    }

    /// Returns the hash for this cache
    pub(crate) const fn executed_block_hash(&self) -> B256 {
        self.hash
    }

    /// Splits the cache into its caches and metrics, consuming it.
    pub(crate) fn split(self) -> (ProviderCaches, CachedStateMetrics) {
        (self.caches, self.metrics)
    }

    /// Returns the [`ProviderCaches`] belonging to the tracked hash.
    pub(crate) const fn cache(&self) -> &ProviderCaches {
        &self.caches
    }

    /// Updates the metrics for the [`ProviderCaches`].
    pub(crate) fn update_metrics(&self) {
        self.metrics.storage_cache_size.set(self.caches.total_storage_slots() as f64);
        self.metrics.account_cache_size.set(self.caches.account_cache.entry_count() as f64);
        self.metrics.code_cache_size.set(self.caches.code_cache.entry_count() as f64);
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

    /// Get a storage value from this account's cache.
    /// - `NotCached`: The slot is not in the cache
    /// - `Empty`: The slot is empty
    /// - `Value`: The slot has a specific value
    pub(crate) fn get_storage(&self, key: &StorageKey) -> SlotStatus {
        match self.slots.get(key) {
            None => SlotStatus::NotCached,
            Some(None) => SlotStatus::Empty,
            Some(Some(value)) => SlotStatus::Value(value),
        }
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
    use alloy_primitives::{B256, U256};
    use rand::Rng;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
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
        let mut rng = rand::rng();

        let key = StorageKey::random();
        let value = StorageValue::from(rng.random::<u128>());
        let (first_slot, _) = measure_allocation(|| {
            cache.insert_storage(key, Some(value));
        });
        println!("First slot insertion overhead: {} bytes", first_slot);

        const TOTAL_SLOTS: usize = 10_000;
        let (test_slots, _) = measure_allocation(|| {
            for _ in 0..TOTAL_SLOTS {
                let key = StorageKey::random();
                let value = StorageValue::from(rng.random::<u128>());
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

    #[test]
    fn test_empty_storage_cached_state_provider() {
        // make sure when we have an empty value in storage, we return `Empty` and not `NotCached`
        let address = Address::random();
        let storage_key = StorageKey::random();
        let account = ExtendedAccount::new(0, U256::ZERO);

        // note there is no storage here
        let provider = MockEthProvider::default();
        provider.extend_accounts(vec![(address, account)]);

        let caches = ProviderCacheBuilder::default().build_caches(1000);
        let state_provider =
            CachedStateProvider::new_with_caches(provider, caches, CachedStateMetrics::zeroed());

        // check that the storage is empty
        let res = state_provider.storage(address, storage_key);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), None);
    }

    #[test]
    fn test_uncached_storage_cached_state_provider() {
        // make sure when we have something uncached, we get the cached value
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);
        let account =
            ExtendedAccount::new(0, U256::ZERO).extend_storage(vec![(storage_key, storage_value)]);

        // note that we extend storage here with one value
        let provider = MockEthProvider::default();
        provider.extend_accounts(vec![(address, account)]);

        let caches = ProviderCacheBuilder::default().build_caches(1000);
        let state_provider =
            CachedStateProvider::new_with_caches(provider, caches, CachedStateMetrics::zeroed());

        // check that the storage is empty
        let res = state_provider.storage(address, storage_key);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), Some(storage_value));
    }

    #[test]
    fn test_get_storage_populated() {
        // make sure when we have something cached, we get the cached value in the `SlotStatus`
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);

        // insert into caches directly
        let caches = ProviderCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, Some(storage_value));

        // check that the storage is empty
        let slot_status = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::Value(storage_value));
    }

    #[test]
    fn test_get_storage_not_cached() {
        // make sure when we have nothing cached, we get the `NotCached` value in the `SlotStatus`
        let storage_key = StorageKey::random();
        let address = Address::random();

        // just create empty caches
        let caches = ProviderCacheBuilder::default().build_caches(1000);

        // check that the storage is empty
        let slot_status = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::NotCached);
    }

    #[test]
    fn test_get_storage_empty() {
        // make sure when we insert an empty value to the cache, we get the `Empty` value in the
        // `SlotStatus`
        let address = Address::random();
        let storage_key = StorageKey::random();

        // insert into caches directly
        let caches = ProviderCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, None);

        // check that the storage is empty
        let slot_status = caches.get_storage(&address, &storage_key);
        assert_eq!(slot_status, SlotStatus::Empty);
    }
}
