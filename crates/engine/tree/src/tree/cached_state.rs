//! Execution cache implementation for block processing.
use alloy_primitives::{
    map::{DefaultHashBuilder, FbBuildHasher},
    Address, StorageKey, StorageValue, B256,
};
use metrics::Gauge;
use mini_moka::sync::CacheBuilder;
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_revm::db::BundleState;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tracing::{debug_span, instrument, trace};

/// Type alias for the mini-moka cache used for bytecode.
pub(crate) type MokaCache<K, V, H = DefaultHashBuilder> = mini_moka::sync::Cache<K, V, H>;

/// Type alias for the fixed-cache used for accounts and storage.
pub(crate) type FixedCache<K, V, H = DefaultHashBuilder> = fixed_cache::Cache<K, V, H>;

/// A wrapper of a state provider and a shared cache.
pub(crate) struct CachedStateProvider<S> {
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
    pub(crate) const fn new(
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
    /// is then updated after the block with the entire [`BundleState`] output of that block which
    /// contains all accessed accounts,code,storage. See also [`ExecutionCache::insert_state`].
    pub(crate) const fn prewarm(mut self) -> Self {
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
pub(crate) enum SlotStatus {
    /// The storage slot is not in the cache (or was invalidated).
    NotCached(StorageValue),
    /// The storage slot exists in cache and has a specific value.
    Value(StorageValue),
}

impl<S: StateProvider> StateProvider for CachedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.caches.get_or_try_insert_storage_with(account, storage_key, || {
            self.state_provider.storage(account, storage_key).map(Option::unwrap_or_default)
        })? {
            SlotStatus::NotCached(value) => {
                self.metrics.storage_cache_misses.increment(1);
                Ok(Some(value).filter(|v| !v.is_zero()))
            }
            SlotStatus::Value(value) => {
                self.metrics.storage_cache_hits.increment(1);
                Ok(Some(value).filter(|v| !v.is_zero()))
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

/// Storage entry with timestamp for invalidation tracking.
/// The timestamp is used to detect stale entries after account destruction.
#[derive(Debug, Clone, Copy)]
struct TimestampedStorage {
    /// Timestamp when this entry was inserted
    insert_ts: u64,
    /// The storage value
    value: StorageValue,
}

/// Execution cache used during block processing.
///
/// Optimizes state access by maintaining in-memory copies of frequently accessed
/// accounts, storage slots, and bytecode. Works in conjunction with prewarming
/// to reduce database I/O during block execution.
///
/// ## Storage Invalidation
///
/// When an account is destroyed (SELFDESTRUCT), all its storage must be invalidated.
/// This is handled using timestamps:
/// - Each storage entry stores the timestamp when it was inserted
/// - Each account tracks when it was last wiped (destroyed)
/// - On lookup, if the entry's timestamp <= wipe timestamp, the entry is stale
#[derive(Debug, Clone)]
pub(crate) struct ExecutionCache {
    /// Cache for contract bytecode, keyed by code hash.
    /// Uses mini-moka for LRU eviction since bytecode is variable-sized.
    code_cache: MokaCache<B256, Option<Bytecode>, FbBuildHasher<32>>,

    /// Flat storage cache: maps `(Address, StorageKey)` to timestamped storage value.
    /// Uses fixed-cache for lock-free access with no eviction.
    storage_cache: Arc<FixedCache<(Address, StorageKey), TimestampedStorage>>,

    /// Wipe timestamps: tracks when each account was last destroyed.
    /// Used to invalidate stale storage entries.
    wipe_cache: Arc<FixedCache<Address, u64, FbBuildHasher<20>>>,

    /// Global counter for generating timestamps.
    /// Incremented on every insert and wipe operation.
    counter: Arc<AtomicU64>,

    /// Cache for basic account information (nonce, balance, code hash).
    /// Uses fixed-cache for lock-free access with no eviction.
    account_cache: Arc<FixedCache<Address, Option<Account>, FbBuildHasher<20>>>,
}

impl ExecutionCache {
    /// Gets the next timestamp from the global counter.
    fn next_timestamp(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Gets the wipe timestamp for an address (0 if never wiped).
    fn get_wipe_timestamp(&self, address: &Address) -> u64 {
        self.wipe_cache.get(address).unwrap_or(0)
    }

    /// Insert storage value into cache with current timestamp.
    pub(crate) fn insert_storage(
        &self,
        address: Address,
        key: StorageKey,
        value: Option<StorageValue>,
    ) {
        let ts = self.next_timestamp();
        let entry = TimestampedStorage { insert_ts: ts, value: value.unwrap_or_default() };
        self.storage_cache.insert((address, key), entry);
    }

    /// Insert multiple storage values into cache for a single account.
    pub(crate) fn insert_storage_bulk<I>(&self, address: Address, storage_entries: I)
    where
        I: IntoIterator<Item = (StorageKey, Option<StorageValue>)>,
    {
        for (key, value) in storage_entries {
            self.insert_storage(address, key, value)
        }
    }

    /// Invalidates all storage for an account by recording a wipe timestamp.
    /// Any cached storage entries with `insert_ts <= wipe_ts` are considered stale.
    pub(crate) fn invalidate_account_storage(&self, address: Address) {
        let ts = self.next_timestamp();
        self.wipe_cache.insert(address, ts);
    }

    /// Gets storage from cache, or inserts using the provided function.
    /// Returns `SlotStatus::NotCached` if the value was freshly fetched,
    /// or `SlotStatus::Value` if it was already cached and valid.
    pub(crate) fn get_or_try_insert_storage_with<E>(
        &self,
        address: Address,
        key: StorageKey,
        f: impl FnOnce() -> Result<StorageValue, E>,
    ) -> Result<SlotStatus, E> {
        let wipe_ts = self.get_wipe_timestamp(&address);

        // Check if we have a valid cached entry
        if let Some(entry) = self.storage_cache.get(&(address, key)) &&
            entry.insert_ts > wipe_ts
        {
            // Entry is valid (inserted after last wipe)
            return Ok(SlotStatus::Value(entry.value));
        }

        // Cache miss or stale entry - fetch from provider
        let value = f()?;
        let ts = self.next_timestamp();
        let entry = TimestampedStorage { insert_ts: ts, value };
        self.storage_cache.insert((address, key), entry);

        Ok(SlotStatus::NotCached(value))
    }

    /// Inserts the post-execution state changes into the cache.
    ///
    /// This method is called after transaction execution to update the cache with
    /// the touched and modified state. The insertion order is critical:
    ///
    /// 1. Bytecodes: Insert contract code first
    /// 2. Storage slots: Update storage values for each account
    /// 3. Accounts: Update account info (nonce, balance, code hash)
    ///
    /// ## Why This Order Matters
    ///
    /// Account information references bytecode via code hash. If we update accounts
    /// before bytecode, we might create cache entries pointing to non-existent code.
    /// The current order ensures cache consistency.
    ///
    /// ## Error Handling
    ///
    /// Returns an error if the state updates are inconsistent and should be discarded.
    #[instrument(level = "debug", target = "engine::caching", skip_all)]
    pub(crate) fn insert_state(&self, state_updates: &BundleState) -> Result<(), ()> {
        let _enter =
            debug_span!(target: "engine::tree", "contracts", len = state_updates.contracts.len())
                .entered();
        // Insert bytecodes
        for (code_hash, bytecode) in &state_updates.contracts {
            self.code_cache.insert(*code_hash, Some(Bytecode(bytecode.clone())));
        }
        drop(_enter);

        let _enter = debug_span!(
            target: "engine::tree",
            "accounts",
            accounts = state_updates.state.len(),
            storages =
                state_updates.state.values().map(|account| account.storage.len()).sum::<usize>()
        )
        .entered();
        for (addr, account) in &state_updates.state {
            // If the account was not modified, as in not changed and not destroyed, then we have
            // nothing to do w.r.t. this particular account and can move on
            if account.status.is_not_modified() {
                continue
            }

            // If the account was destroyed, invalidate from the account / storage caches
            if account.was_destroyed() {
                // For account cache, we insert None to indicate destroyed
                self.account_cache.insert(*addr, None);

                // Invalidate all storage for this account
                self.invalidate_account_storage(*addr);
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
            let storage_entries = account
                .storage
                .iter()
                .map(|(storage_key, slot)| ((*storage_key).into(), Some(slot.present_value)));
            self.insert_storage_bulk(*addr, storage_entries);

            // Insert will update if present, so we just use the new account info as the new value
            // for the account cache
            self.account_cache.insert(*addr, Some(Account::from(account_info)));
        }

        Ok(())
    }
}

/// A builder for [`ExecutionCache`].
#[derive(Debug)]
pub(crate) struct ExecutionCacheBuilder {
    /// Code cache entries (for mini-moka)
    code_cache_entries: u64,

    /// Storage cache entries (for fixed-cache)
    storage_cache_entries: usize,

    /// Account cache entries (for fixed-cache)
    account_cache_entries: usize,

    /// Wipe cache entries (for tracking destroyed accounts)
    wipe_cache_entries: usize,
}

impl ExecutionCacheBuilder {
    /// Build an [`ExecutionCache`] struct, so that execution caches can be easily cloned.
    pub(crate) fn build_caches(self, total_cache_size: u64) -> ExecutionCache {
        use std::time::Duration;

        const EXPIRY_TIME: Duration = Duration::from_secs(7200); // 2 hours
        const TIME_TO_IDLE: Duration = Duration::from_secs(3600); // 1 hour

        // Code cache uses mini-moka with weigher for variable-sized bytecode
        let code_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total
        let code_cache = CacheBuilder::new(self.code_cache_entries)
            .weigher(|_key: &B256, value: &Option<Bytecode>| -> u32 {
                let code_size = match value {
                    Some(bytecode) => {
                        (size_of_val(value) +
                            bytecode.bytecode().len() +
                            bytecode
                                .legacy_jump_table()
                                .map(|table| table.as_slice().len())
                                .unwrap_or_default()) as u32
                    }
                    None => size_of_val(value) as u32,
                };
                32 + code_size
            })
            .max_capacity(code_cache_size)
            .time_to_live(EXPIRY_TIME)
            .time_to_idle(TIME_TO_IDLE)
            .build_with_hasher(FbBuildHasher::<32>::default());

        // Storage cache uses fixed-cache (no eviction, lock-free)
        let storage_cache =
            Arc::new(FixedCache::new(self.storage_cache_entries, DefaultHashBuilder::default()));

        // Wipe cache tracks destroyed accounts
        let wipe_cache =
            Arc::new(FixedCache::new(self.wipe_cache_entries, FbBuildHasher::<20>::default()));

        // Global counter for timestamps
        let counter = Arc::new(AtomicU64::new(1));

        // Account cache uses fixed-cache (no eviction, lock-free)
        let account_cache =
            Arc::new(FixedCache::new(self.account_cache_entries, FbBuildHasher::<20>::default()));

        ExecutionCache { code_cache, storage_cache, wipe_cache, counter, account_cache }
    }
}

impl Default for ExecutionCacheBuilder {
    fn default() -> Self {
        // Fixed-cache requires power-of-two sizes
        // Storage: 16M entries for (Address, StorageKey) pairs
        // Account: 4M entries for addresses
        // Wipe: 1M entries for destroyed accounts
        Self {
            code_cache_entries: 10_000_000,
            storage_cache_entries: 16 * 1024 * 1024, // 16M, power of 2
            account_cache_entries: 4 * 1024 * 1024,  // 4M, power of 2
            wipe_cache_entries: 1024 * 1024,         // 1M, power of 2
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
    caches: ExecutionCache,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,

    /// A guard to track in-flight usage of this cache.
    /// The cache is considered available if the strong count is 1.
    usage_guard: Arc<()>,
}

impl SavedCache {
    /// Creates a new instance with the internals
    pub(super) fn new(hash: B256, caches: ExecutionCache, metrics: CachedStateMetrics) -> Self {
        Self { hash, caches, metrics, usage_guard: Arc::new(()) }
    }

    /// Returns the hash for this cache
    pub(crate) const fn executed_block_hash(&self) -> B256 {
        self.hash
    }

    /// Splits the cache into its caches and metrics, consuming it.
    pub(crate) fn split(self) -> (ExecutionCache, CachedStateMetrics) {
        (self.caches, self.metrics)
    }

    /// Returns true if the cache is available for use (no other tasks are currently using it).
    pub(crate) fn is_available(&self) -> bool {
        Arc::strong_count(&self.usage_guard) == 1
    }

    /// Returns the current strong count of the usage guard.
    pub(crate) fn usage_count(&self) -> usize {
        Arc::strong_count(&self.usage_guard)
    }

    /// Returns the [`ExecutionCache`] belonging to the tracked hash.
    pub(crate) const fn cache(&self) -> &ExecutionCache {
        &self.caches
    }

    /// Returns the metrics associated with this cache.
    pub(crate) const fn metrics(&self) -> &CachedStateMetrics {
        &self.metrics
    }

    /// Updates the metrics for the [`ExecutionCache`].
    pub(crate) fn update_metrics(&self) {
        // fixed-cache doesn't provide entry_count, so we can't track size accurately
        // We could track inserts manually if needed
        self.metrics.code_cache_size.set(self.caches.code_cache.entry_count() as f64);
    }
}

#[cfg(test)]
impl SavedCache {
    fn clone_guard_for_test(&self) -> Arc<()> {
        self.usage_guard.clone()
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

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
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

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
        let state_provider =
            CachedStateProvider::new(provider, caches, CachedStateMetrics::zeroed());

        let res = state_provider.storage(address, storage_key);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), Some(storage_value));
    }

    #[test]
    fn test_storage_invalidation_on_destroy() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(42);

        let caches = ExecutionCacheBuilder::default().build_caches(1000);

        // Insert a storage value
        caches.insert_storage(address, storage_key, Some(storage_value));

        // Verify it's cached
        let result = caches.get_or_try_insert_storage_with(address, storage_key, || {
            Ok::<_, ()>(U256::from(999)) // Should not be called
        });
        assert_eq!(result.unwrap(), SlotStatus::Value(storage_value));

        // Invalidate the account's storage (simulating SELFDESTRUCT)
        caches.invalidate_account_storage(address);

        // Now the cached value should be stale, so the fallback should be called
        let result = caches.get_or_try_insert_storage_with(address, storage_key, || {
            Ok::<_, ()>(U256::from(0)) // This should be called now
        });
        assert_eq!(result.unwrap(), SlotStatus::NotCached(U256::ZERO));
    }

    #[test]
    fn test_storage_after_destroy_and_recreate() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let old_value = U256::from(100);
        let new_value = U256::from(200);

        let caches = ExecutionCacheBuilder::default().build_caches(1000);

        // Insert old storage value
        caches.insert_storage(address, storage_key, Some(old_value));

        // Destroy the account
        caches.invalidate_account_storage(address);

        // Insert new storage value (account recreated with new storage)
        caches.insert_storage(address, storage_key, Some(new_value));

        // Should get the new value, not the old one
        let result = caches.get_or_try_insert_storage_with(address, storage_key, || {
            Ok::<_, ()>(U256::from(999)) // Should not be called
        });
        assert_eq!(result.unwrap(), SlotStatus::Value(new_value));
    }

    #[test]
    fn test_get_storage_populated() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, Some(storage_value));

        let result = caches
            .get_or_try_insert_storage_with(address, storage_key, || Ok::<_, ()>(U256::from(999)));
        assert_eq!(result.unwrap(), SlotStatus::Value(storage_value));
    }

    #[test]
    fn test_get_storage_empty() {
        let address = Address::random();
        let storage_key = StorageKey::random();

        let caches = ExecutionCacheBuilder::default().build_caches(1000);
        caches.insert_storage(address, storage_key, None);

        let result = caches
            .get_or_try_insert_storage_with(address, storage_key, || Ok::<_, ()>(U256::from(999)));
        assert_eq!(result.unwrap(), SlotStatus::Value(U256::ZERO));
    }

    #[test]
    fn test_saved_cache_is_available() {
        let execution_cache = ExecutionCacheBuilder::default().build_caches(1000);
        let cache = SavedCache::new(B256::ZERO, execution_cache, CachedStateMetrics::zeroed());

        assert!(cache.is_available(), "Cache should be available initially");

        let _guard = cache.clone_guard_for_test();

        assert!(!cache.is_available(), "Cache should not be available with active guard");
    }

    #[test]
    fn test_saved_cache_multiple_references() {
        let execution_cache = ExecutionCacheBuilder::default().build_caches(1000);
        let cache =
            SavedCache::new(B256::from([2u8; 32]), execution_cache, CachedStateMetrics::zeroed());

        let guard1 = cache.clone_guard_for_test();
        let guard2 = cache.clone_guard_for_test();
        let guard3 = guard1.clone();

        assert!(!cache.is_available());

        drop(guard1);
        assert!(!cache.is_available());

        drop(guard2);
        assert!(!cache.is_available());

        drop(guard3);
        assert!(cache.is_available());
    }
}
