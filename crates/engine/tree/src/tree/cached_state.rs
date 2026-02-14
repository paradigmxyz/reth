//! Execution cache implementation for block processing.
use alloy_primitives::{
    map::{DefaultHashBuilder, FbBuildHasher},
    Address, StorageKey, StorageValue, B256,
};
use fixed_cache::{AnyRef, CacheConfig, Stats, StatsHandler};
use metrics::{Counter, Gauge, Histogram};
use parking_lot::Once;
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
use revm_primitives::eip7907::MAX_CODE_SIZE;
use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{debug_span, instrument, trace, warn};

/// Alignment in bytes for entries in the fixed-cache.
///
/// Each bucket in `fixed-cache` is aligned to 128 bytes (cache line) due to
/// `#[repr(C, align(128))]` on the internal `Bucket` struct.
const FIXED_CACHE_ALIGNMENT: usize = 128;

/// Overhead per entry in the fixed-cache (the `AtomicUsize` tag field).
const FIXED_CACHE_ENTRY_OVERHEAD: usize = size_of::<usize>();

/// Calculates the actual size of a fixed-cache entry for a given key-value pair.
///
/// The entry size is `overhead + size_of::<K>() + size_of::<V>()`, rounded up to the
/// next multiple of [`FIXED_CACHE_ALIGNMENT`] (128 bytes).
const fn fixed_cache_entry_size<K, V>() -> usize {
    fixed_cache_key_size_with_value::<K>(size_of::<V>())
}

/// Calculates the actual size of a fixed-cache entry for a given key-value pair.
///
/// The entry size is `overhead + size_of::<K>() + size_of::<V>()`, rounded up to the
/// next multiple of [`FIXED_CACHE_ALIGNMENT`] (128 bytes).
const fn fixed_cache_key_size_with_value<K>(value: usize) -> usize {
    let raw_size = FIXED_CACHE_ENTRY_OVERHEAD + size_of::<K>() + value;
    // Round up to next multiple of alignment
    raw_size.div_ceil(FIXED_CACHE_ALIGNMENT) * FIXED_CACHE_ALIGNMENT
}

/// Size in bytes of a single code cache entry.
const CODE_CACHE_ENTRY_SIZE: usize = fixed_cache_key_size_with_value::<Address>(MAX_CODE_SIZE);

/// Size in bytes of a single storage cache entry.
const STORAGE_CACHE_ENTRY_SIZE: usize =
    fixed_cache_entry_size::<(Address, StorageKey), StorageValue>();

/// Size in bytes of a single account cache entry.
const ACCOUNT_CACHE_ENTRY_SIZE: usize = fixed_cache_entry_size::<Address, Option<Account>>();

/// Cache configuration with epoch tracking enabled for O(1) cache invalidation.
struct EpochCacheConfig;
impl CacheConfig for EpochCacheConfig {
    const EPOCHS: bool = true;
}

/// Type alias for the fixed-cache used for accounts and storage.
type FixedCache<K, V, H = DefaultHashBuilder> = fixed_cache::Cache<K, V, H, EpochCacheConfig>;

/// A wrapper of a state provider and a shared cache.
///
/// The const generic `PREWARM` controls whether every cache miss is populated. This is only
/// relevant for pre-warm transaction execution with the intention to pre-populate the cache with
/// data for regular block execution. During regular block execution the cache doesn't need to be
/// populated because the actual EVM database [`State`](revm::database::State) also caches
/// internally during block execution and the cache is then updated after the block with the entire
/// [`BundleState`] output of that block which contains all accessed accounts, code, storage. See
/// also [`ExecutionCache::insert_state`].
#[derive(Debug)]
pub struct CachedStateProvider<S, const PREWARM: bool = false> {
    /// The state provider
    state_provider: S,

    /// The caches used for the provider
    caches: ExecutionCache,

    /// Metrics for the cached state provider
    metrics: CachedStateMetrics,
}

impl<S> CachedStateProvider<S> {
    /// Creates a new [`CachedStateProvider`] from an [`ExecutionCache`], state provider, and
    /// [`CachedStateMetrics`].
    pub const fn new(
        state_provider: S,
        caches: ExecutionCache,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics }
    }
}

impl<S> CachedStateProvider<S, true> {
    /// Creates a new [`CachedStateProvider`] with prewarming enabled.
    pub const fn new_prewarm(
        state_provider: S,
        caches: ExecutionCache,
        metrics: CachedStateMetrics,
    ) -> Self {
        Self { state_provider, caches, metrics }
    }
}

/// Metrics for the cached state provider, showing hits / misses / size for each cache.
///
/// This struct combines both the provider-level metrics (hits/misses tracked by the provider)
/// and the fixed-cache internal stats (collisions, size, capacity).
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub struct CachedStateMetrics {
    /// Number of times a new execution cache was created
    execution_cache_created_total: Counter,

    /// Duration of execution cache creation in seconds
    execution_cache_creation_duration_seconds: Histogram,

    /// Code cache hits
    code_cache_hits: Gauge,

    /// Code cache misses
    code_cache_misses: Gauge,

    /// Code cache size (number of entries)
    code_cache_size: Gauge,

    /// Code cache capacity (maximum entries)
    code_cache_capacity: Gauge,

    /// Code cache collisions (hash collisions causing eviction)
    code_cache_collisions: Gauge,

    /// Storage cache hits
    storage_cache_hits: Gauge,

    /// Storage cache misses
    storage_cache_misses: Gauge,

    /// Storage cache size (number of entries)
    storage_cache_size: Gauge,

    /// Storage cache capacity (maximum entries)
    storage_cache_capacity: Gauge,

    /// Storage cache collisions (hash collisions causing eviction)
    storage_cache_collisions: Gauge,

    /// Account cache hits
    account_cache_hits: Gauge,

    /// Account cache misses
    account_cache_misses: Gauge,

    /// Account cache size (number of entries)
    account_cache_size: Gauge,

    /// Account cache capacity (maximum entries)
    account_cache_capacity: Gauge,

    /// Account cache collisions (hash collisions causing eviction)
    account_cache_collisions: Gauge,
}

impl CachedStateMetrics {
    /// Sets all values to zero, indicating that a new block is being executed.
    pub fn reset(&self) {
        // code cache
        self.code_cache_hits.set(0);
        self.code_cache_misses.set(0);
        self.code_cache_collisions.set(0);

        // storage cache
        self.storage_cache_hits.set(0);
        self.storage_cache_misses.set(0);
        self.storage_cache_collisions.set(0);

        // account cache
        self.account_cache_hits.set(0);
        self.account_cache_misses.set(0);
        self.account_cache_collisions.set(0);
    }

    /// Returns a new zeroed-out instance of [`CachedStateMetrics`].
    pub fn zeroed() -> Self {
        let zeroed = Self::default();
        zeroed.reset();
        zeroed
    }

    /// Records a new execution cache creation with its duration.
    pub(crate) fn record_cache_creation(&self, duration: Duration) {
        self.execution_cache_created_total.increment(1);
        self.execution_cache_creation_duration_seconds.record(duration.as_secs_f64());
    }
}

/// A stats handler for fixed-cache that tracks collisions and size.
///
/// Note: Hits and misses are tracked directly by the [`CachedStateProvider`] via
/// [`CachedStateMetrics`], not here. The stats handler is used for:
/// - Collision detection (hash collisions causing eviction of a different key)
/// - Size tracking
///
/// ## Size Tracking
///
/// Size is tracked via `on_insert` and `on_remove` callbacks:
/// - `on_insert`: increment size only when inserting into an empty bucket (no eviction)
/// - `on_remove`: always decrement size
///
/// Collisions (evicting a different key) don't change size since they replace an existing entry.
#[derive(Debug)]
pub(crate) struct CacheStatsHandler {
    collisions: AtomicU64,
    size: AtomicUsize,
    capacity: usize,
}

impl CacheStatsHandler {
    /// Creates a new stats handler with all counters initialized to zero.
    pub(crate) const fn new(capacity: usize) -> Self {
        Self { collisions: AtomicU64::new(0), size: AtomicUsize::new(0), capacity }
    }

    /// Returns the number of cache collisions.
    pub(crate) fn collisions(&self) -> u64 {
        self.collisions.load(Ordering::Relaxed)
    }

    /// Returns the current size (number of entries).
    pub(crate) fn size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// Returns the capacity (maximum number of entries).
    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Increments the size counter. Called on cache insert.
    pub(crate) fn increment_size(&self) {
        let _ = self.size.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the size counter. Called on cache remove.
    pub(crate) fn decrement_size(&self) {
        let _ = self.size.fetch_sub(1, Ordering::Relaxed);
    }

    /// Resets size to zero. Called on cache clear.
    pub(crate) fn reset_size(&self) {
        self.size.store(0, Ordering::Relaxed);
    }

    /// Resets collision counter to zero (but not size).
    pub(crate) fn reset_stats(&self) {
        self.collisions.store(0, Ordering::Relaxed);
    }
}

impl<K: PartialEq, V> StatsHandler<K, V> for CacheStatsHandler {
    fn on_hit(&self, _key: &K, _value: &V) {}

    fn on_miss(&self, _key: AnyRef<'_>) {}

    fn on_insert(&self, key: &K, _value: &V, evicted: Option<(&K, &V)>) {
        match evicted {
            None => {
                // Inserting into an empty bucket
                self.increment_size();
            }
            Some((evicted_key, _)) if evicted_key != key => {
                // Collision: evicting a different key
                self.collisions.fetch_add(1, Ordering::Relaxed);
            }
            Some(_) => {
                // Updating the same key, size unchanged
            }
        }
    }

    fn on_remove(&self, _key: &K, _value: &V) {
        self.decrement_size();
    }
}

impl<S: AccountReader, const PREWARM: bool> AccountReader for CachedStateProvider<S, PREWARM> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if PREWARM {
            match self.caches.get_or_try_insert_account_with(*address, || {
                self.state_provider.basic_account(address)
            })? {
                CachedStatus::NotCached(value) | CachedStatus::Cached(value) => Ok(value),
            }
        } else if let Some(account) = self.caches.0.account_cache.get(address) {
            self.metrics.account_cache_hits.increment(1);
            Ok(account)
        } else {
            self.metrics.account_cache_misses.increment(1);
            self.state_provider.basic_account(address)
        }
    }
}

/// Represents the status of a key in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachedStatus<T> {
    /// The key is not in the cache (or was invalidated). The value was recalculated.
    NotCached(T),
    /// The key exists in cache and has a specific value.
    Cached(T),
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
                CachedStatus::NotCached(value) | CachedStatus::Cached(value) => {
                    // The slot that was never written to is indistinguishable from a slot
                    // explicitly set to zero. We return `None` in both cases.
                    Ok(Some(value).filter(|v| !v.is_zero()))
                }
            }
        } else if let Some(value) = self.caches.0.storage_cache.get(&(account, storage_key)) {
            self.metrics.storage_cache_hits.increment(1);
            Ok(Some(value).filter(|v| !v.is_zero()))
        } else {
            self.metrics.storage_cache_misses.increment(1);
            self.state_provider.storage(account, storage_key)
        }
    }

    fn storage_by_hashed_key(
        &self,
        address: Address,
        hashed_storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        self.state_provider.storage_by_hashed_key(address, hashed_storage_key)
    }
}

impl<S: BytecodeReader, const PREWARM: bool> BytecodeReader for CachedStateProvider<S, PREWARM> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if PREWARM {
            match self.caches.get_or_try_insert_code_with(*code_hash, || {
                self.state_provider.bytecode_by_hash(code_hash)
            })? {
                CachedStatus::NotCached(code) | CachedStatus::Cached(code) => Ok(code),
            }
        } else if let Some(code) = self.caches.0.code_cache.get(code_hash) {
            self.metrics.code_cache_hits.increment(1);
            Ok(code)
        } else {
            self.metrics.code_cache_misses.increment(1);
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

/// Execution cache used during block processing.
///
/// Optimizes state access by maintaining in-memory copies of frequently accessed
/// accounts, storage slots, and bytecode. Works in conjunction with prewarming
/// to reduce database I/O during block execution.
///
/// ## Storage Invalidation
///
/// Since EIP-6780, SELFDESTRUCT only works within the same transaction where the
/// contract was created, so we don't need to handle clearing the storage.
#[derive(Debug, Clone)]
pub struct ExecutionCache(Arc<ExecutionCacheInner>);

/// Inner state of the [`ExecutionCache`], wrapped in a single [`Arc`].
#[derive(Debug)]
struct ExecutionCacheInner {
    /// Cache for contract bytecode, keyed by code hash.
    code_cache: FixedCache<B256, Option<Bytecode>, FbBuildHasher<32>>,

    /// Flat storage cache: maps `(Address, StorageKey)` to storage value.
    storage_cache: FixedCache<(Address, StorageKey), StorageValue>,

    /// Cache for basic account information (nonce, balance, code hash).
    account_cache: FixedCache<Address, Option<Account>, FbBuildHasher<20>>,

    /// Stats handler for the code cache (shared with the cache via [`Stats`]).
    code_stats: Arc<CacheStatsHandler>,

    /// Stats handler for the storage cache (shared with the cache via [`Stats`]).
    storage_stats: Arc<CacheStatsHandler>,

    /// Stats handler for the account cache (shared with the cache via [`Stats`]).
    account_stats: Arc<CacheStatsHandler>,

    /// One-time notification when SELFDESTRUCT is encountered
    selfdestruct_encountered: Once,
}

impl ExecutionCache {
    /// Minimum cache size required when epochs are enabled.
    /// With EPOCHS=true, fixed-cache requires 12 bottom bits to be zero (2 needed + 10 epoch).
    const MIN_CACHE_SIZE_WITH_EPOCHS: usize = 1 << 12; // 4096

    /// Converts a byte size to number of cache entries, rounding down to a power of two.
    ///
    /// Fixed-cache requires power-of-two sizes for efficient indexing.
    /// With epochs enabled, the minimum size is 4096 entries.
    pub const fn bytes_to_entries(size_bytes: usize, entry_size: usize) -> usize {
        let entries = size_bytes / entry_size;
        // Round down to nearest power of two
        let rounded = if entries == 0 { 1 } else { (entries + 1).next_power_of_two() >> 1 };
        // Ensure minimum size for epoch tracking
        if rounded < Self::MIN_CACHE_SIZE_WITH_EPOCHS {
            Self::MIN_CACHE_SIZE_WITH_EPOCHS
        } else {
            rounded
        }
    }

    /// Build an [`ExecutionCache`] struct, so that execution caches can be easily cloned.
    pub fn new(total_cache_size: usize) -> Self {
        let code_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total
        let storage_cache_size = (total_cache_size * 8888) / 10000; // 88.88% of total
        let account_cache_size = (total_cache_size * 556) / 10000; // 5.56% of total

        let code_capacity = Self::bytes_to_entries(code_cache_size, CODE_CACHE_ENTRY_SIZE);
        let storage_capacity = Self::bytes_to_entries(storage_cache_size, STORAGE_CACHE_ENTRY_SIZE);
        let account_capacity = Self::bytes_to_entries(account_cache_size, ACCOUNT_CACHE_ENTRY_SIZE);

        let code_stats = Arc::new(CacheStatsHandler::new(code_capacity));
        let storage_stats = Arc::new(CacheStatsHandler::new(storage_capacity));
        let account_stats = Arc::new(CacheStatsHandler::new(account_capacity));

        Self(Arc::new(ExecutionCacheInner {
            code_cache: FixedCache::new(code_capacity, FbBuildHasher::<32>::default())
                .with_stats(Some(Stats::new(code_stats.clone()))),
            storage_cache: FixedCache::new(storage_capacity, DefaultHashBuilder::default())
                .with_stats(Some(Stats::new(storage_stats.clone()))),
            account_cache: FixedCache::new(account_capacity, FbBuildHasher::<20>::default())
                .with_stats(Some(Stats::new(account_stats.clone()))),
            code_stats,
            storage_stats,
            account_stats,
            selfdestruct_encountered: Once::new(),
        }))
    }

    /// Gets code from cache, or inserts using the provided function.
    pub fn get_or_try_insert_code_with<E>(
        &self,
        hash: B256,
        f: impl FnOnce() -> Result<Option<Bytecode>, E>,
    ) -> Result<CachedStatus<Option<Bytecode>>, E> {
        let mut miss = false;
        let result = self.0.code_cache.get_or_try_insert_with(hash, |_| {
            miss = true;
            f()
        })?;

        if miss {
            Ok(CachedStatus::NotCached(result))
        } else {
            Ok(CachedStatus::Cached(result))
        }
    }

    /// Gets storage from cache, or inserts using the provided function.
    pub fn get_or_try_insert_storage_with<E>(
        &self,
        address: Address,
        key: StorageKey,
        f: impl FnOnce() -> Result<StorageValue, E>,
    ) -> Result<CachedStatus<StorageValue>, E> {
        let mut miss = false;
        let result = self.0.storage_cache.get_or_try_insert_with((address, key), |_| {
            miss = true;
            f()
        })?;

        if miss {
            Ok(CachedStatus::NotCached(result))
        } else {
            Ok(CachedStatus::Cached(result))
        }
    }

    /// Gets account from cache, or inserts using the provided function.
    pub fn get_or_try_insert_account_with<E>(
        &self,
        address: Address,
        f: impl FnOnce() -> Result<Option<Account>, E>,
    ) -> Result<CachedStatus<Option<Account>>, E> {
        let mut miss = false;
        let result = self.0.account_cache.get_or_try_insert_with(address, |_| {
            miss = true;
            f()
        })?;

        if miss {
            Ok(CachedStatus::NotCached(result))
        } else {
            Ok(CachedStatus::Cached(result))
        }
    }

    /// Insert storage value into cache.
    pub fn insert_storage(&self, address: Address, key: StorageKey, value: Option<StorageValue>) {
        self.0.storage_cache.insert((address, key), value.unwrap_or_default());
    }

    /// Insert code into cache.
    fn insert_code(&self, hash: B256, code: Option<Bytecode>) {
        self.0.code_cache.insert(hash, code);
    }

    /// Insert account into cache.
    fn insert_account(&self, address: Address, account: Option<Account>) {
        self.0.account_cache.insert(address, account);
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
    #[expect(clippy::result_unit_err)]
    pub fn insert_state(&self, state_updates: &BundleState) -> Result<(), ()> {
        let _enter =
            debug_span!(target: "engine::tree", "contracts", len = state_updates.contracts.len())
                .entered();
        // Insert bytecodes
        for (code_hash, bytecode) in &state_updates.contracts {
            self.insert_code(*code_hash, Some(Bytecode(bytecode.clone())));
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

            // If the original account had code (was a contract), we must clear the entire cache
            // because we can't efficiently invalidate all storage slots for a single address.
            // This should only happen on pre-Dencun networks.
            //
            // If the original account had no code (was an EOA or a not yet deployed contract), we
            // just remove the account from cache - no storage exists for it.
            if account.was_destroyed() {
                let had_code =
                    account.original_info.as_ref().is_some_and(|info| !info.is_empty_code_hash());
                if had_code {
                    self.0.selfdestruct_encountered.call_once(|| {
                        warn!(
                            target: "engine::caching",
                            address = ?addr,
                            info = ?account.info,
                            original_info = ?account.original_info,
                            "Encountered an inter-transaction SELFDESTRUCT that reset the storage cache. Are you running a pre-Dencun network?"
                        );
                    });
                    self.clear();
                    return Ok(())
                }

                self.0.account_cache.remove(addr);
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
            for (key, slot) in &account.storage {
                self.insert_storage(*addr, (*key).into(), Some(slot.present_value));
            }

            // Insert will update if present, so we just use the new account info as the new value
            // for the account cache
            self.insert_account(*addr, Some(Account::from(account_info)));
        }

        Ok(())
    }

    /// Clears storage and account caches, resetting them to empty state.
    ///
    /// We do not clear the bytecodes cache, because its mapping can never change, as it's
    /// `keccak256(bytecode) => bytecode`.
    pub(crate) fn clear(&self) {
        self.0.storage_cache.clear();
        self.0.account_cache.clear();

        self.0.storage_stats.reset_size();
        self.0.account_stats.reset_size();
    }

    /// Updates the provided metrics with the current stats from the cache's stats handlers,
    /// and resets the hit/miss/collision counters.
    pub(crate) fn update_metrics(&self, metrics: &CachedStateMetrics) {
        metrics.code_cache_size.set(self.0.code_stats.size() as f64);
        metrics.code_cache_capacity.set(self.0.code_stats.capacity() as f64);
        metrics.code_cache_collisions.set(self.0.code_stats.collisions() as f64);
        self.0.code_stats.reset_stats();

        metrics.storage_cache_size.set(self.0.storage_stats.size() as f64);
        metrics.storage_cache_capacity.set(self.0.storage_stats.capacity() as f64);
        metrics.storage_cache_collisions.set(self.0.storage_stats.collisions() as f64);
        self.0.storage_stats.reset_stats();

        metrics.account_cache_size.set(self.0.account_stats.size() as f64);
        metrics.account_cache_capacity.set(self.0.account_stats.capacity() as f64);
        metrics.account_cache_collisions.set(self.0.account_stats.collisions() as f64);
        self.0.account_stats.reset_stats();
    }
}

/// A saved cache that has been used for executing a specific block, which has been updated for its
/// execution.
#[derive(Debug, Clone)]
pub struct SavedCache {
    /// The hash of the block these caches were used to execute.
    hash: B256,

    /// The caches used for the provider.
    caches: ExecutionCache,

    /// Metrics for the cached state provider (includes size/capacity/collisions from fixed-cache)
    metrics: CachedStateMetrics,

    /// A guard to track in-flight usage of this cache.
    /// The cache is considered available if the strong count is 1.
    usage_guard: Arc<()>,

    /// Whether to skip cache metrics recording (can be expensive with large cached state).
    disable_cache_metrics: bool,
}

impl SavedCache {
    /// Creates a new instance with the internals
    pub fn new(hash: B256, caches: ExecutionCache, metrics: CachedStateMetrics) -> Self {
        Self { hash, caches, metrics, usage_guard: Arc::new(()), disable_cache_metrics: false }
    }

    /// Sets whether to disable cache metrics recording.
    pub const fn with_disable_cache_metrics(mut self, disable: bool) -> Self {
        self.disable_cache_metrics = disable;
        self
    }

    /// Returns the hash for this cache
    pub const fn executed_block_hash(&self) -> B256 {
        self.hash
    }

    /// Splits the cache into its caches, metrics, and `disable_cache_metrics` flag, consuming it.
    pub fn split(self) -> (ExecutionCache, CachedStateMetrics, bool) {
        (self.caches, self.metrics, self.disable_cache_metrics)
    }

    /// Returns true if the cache is available for use (no other tasks are currently using it).
    pub fn is_available(&self) -> bool {
        Arc::strong_count(&self.usage_guard) == 1
    }

    /// Returns the current strong count of the usage guard.
    pub fn usage_count(&self) -> usize {
        Arc::strong_count(&self.usage_guard)
    }

    /// Returns the [`ExecutionCache`] belonging to the tracked hash.
    pub const fn cache(&self) -> &ExecutionCache {
        &self.caches
    }

    /// Returns the metrics associated with this cache.
    pub const fn metrics(&self) -> &CachedStateMetrics {
        &self.metrics
    }

    /// Updates the cache metrics (size/capacity/collisions) from the stats handlers.
    ///
    /// Note: This can be expensive with large cached state. Use
    /// `with_disable_cache_metrics(true)` to skip.
    pub(crate) fn update_metrics(&self) {
        if self.disable_cache_metrics {
            return
        }
        self.caches.update_metrics(&self.metrics);
    }

    /// Clears all caches, resetting them to empty state.
    pub(crate) fn clear(&self) {
        self.caches.clear();
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
    use alloy_primitives::{map::HashMap, U256};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_revm::db::{AccountStatus, BundleAccount};
    use revm_state::AccountInfo;

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

    #[test]
    fn test_get_storage_populated() {
        let address = Address::random();
        let storage_key = StorageKey::random();
        let storage_value = U256::from(1);

        let caches = ExecutionCache::new(1000);
        caches.insert_storage(address, storage_key, Some(storage_value));

        let result = caches
            .get_or_try_insert_storage_with(address, storage_key, || Ok::<_, ()>(U256::from(999)));
        assert_eq!(result.unwrap(), CachedStatus::Cached(storage_value));
    }

    #[test]
    fn test_get_storage_empty() {
        let address = Address::random();
        let storage_key = StorageKey::random();

        let caches = ExecutionCache::new(1000);
        caches.insert_storage(address, storage_key, None);

        let result = caches
            .get_or_try_insert_storage_with(address, storage_key, || Ok::<_, ()>(U256::from(999)));
        assert_eq!(result.unwrap(), CachedStatus::Cached(U256::ZERO));
    }

    #[test]
    fn test_saved_cache_is_available() {
        let execution_cache = ExecutionCache::new(1000);
        let cache = SavedCache::new(B256::ZERO, execution_cache, CachedStateMetrics::zeroed());

        assert!(cache.is_available(), "Cache should be available initially");

        let _guard = cache.clone_guard_for_test();

        assert!(!cache.is_available(), "Cache should not be available with active guard");
    }

    #[test]
    fn test_saved_cache_multiple_references() {
        let execution_cache = ExecutionCache::new(1000);
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

    #[test]
    fn test_insert_state_destroyed_account_with_code_clears_cache() {
        let caches = ExecutionCache::new(1000);

        // Pre-populate caches with some data
        let addr1 = Address::random();
        let addr2 = Address::random();
        let storage_key = StorageKey::random();
        caches.insert_account(addr1, Some(Account::default()));
        caches.insert_account(addr2, Some(Account::default()));
        caches.insert_storage(addr1, storage_key, Some(U256::from(42)));

        // Verify caches are populated
        assert!(caches.0.account_cache.get(&addr1).is_some());
        assert!(caches.0.account_cache.get(&addr2).is_some());
        assert!(caches.0.storage_cache.get(&(addr1, storage_key)).is_some());

        let bundle = BundleState {
            // BundleState with a destroyed contract (had code)
            state: HashMap::from_iter([(
                Address::random(),
                BundleAccount::new(
                    Some(AccountInfo {
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash: B256::random(), // Non-empty code hash
                        code: None,
                        account_id: None,
                    }),
                    None, // Destroyed, so no current info
                    Default::default(),
                    AccountStatus::Destroyed,
                ),
            )]),
            contracts: Default::default(),
            reverts: Default::default(),
            state_size: 0,
            reverts_size: 0,
        };

        // Insert state should clear all caches because a contract was destroyed
        let result = caches.insert_state(&bundle);
        assert!(result.is_ok());

        // Verify all caches were cleared
        assert!(caches.0.account_cache.get(&addr1).is_none());
        assert!(caches.0.account_cache.get(&addr2).is_none());
        assert!(caches.0.storage_cache.get(&(addr1, storage_key)).is_none());
    }

    #[test]
    fn test_insert_state_destroyed_account_without_code_removes_only_account() {
        let caches = ExecutionCache::new(1000);

        // Pre-populate caches with some data
        let addr1 = Address::random();
        let addr2 = Address::random();
        let storage_key = StorageKey::random();
        caches.insert_account(addr1, Some(Account::default()));
        caches.insert_account(addr2, Some(Account::default()));
        caches.insert_storage(addr1, storage_key, Some(U256::from(42)));

        let bundle = BundleState {
            // BundleState with a destroyed EOA (no code)
            state: HashMap::from_iter([(
                addr1,
                BundleAccount::new(
                    Some(AccountInfo {
                        balance: U256::from(100),
                        nonce: 1,
                        code_hash: alloy_primitives::KECCAK256_EMPTY, // Empty code hash = EOA
                        code: None,
                        account_id: None,
                    }),
                    None, // Destroyed
                    Default::default(),
                    AccountStatus::Destroyed,
                ),
            )]),
            contracts: Default::default(),
            reverts: Default::default(),
            state_size: 0,
            reverts_size: 0,
        };

        // Insert state should only remove the destroyed account
        assert!(caches.insert_state(&bundle).is_ok());

        // Verify only addr1 was removed, other data is still present
        assert!(caches.0.account_cache.get(&addr1).is_none());
        assert!(caches.0.account_cache.get(&addr2).is_some());
        assert!(caches.0.storage_cache.get(&(addr1, storage_key)).is_some());
    }

    #[test]
    fn test_insert_state_destroyed_account_no_original_info_removes_only_account() {
        let caches = ExecutionCache::new(1000);

        // Pre-populate caches
        let addr1 = Address::random();
        let addr2 = Address::random();
        caches.insert_account(addr1, Some(Account::default()));
        caches.insert_account(addr2, Some(Account::default()));

        let bundle = BundleState {
            // BundleState with a destroyed account (has no original info)
            state: HashMap::from_iter([(
                addr1,
                BundleAccount::new(
                    None, // No original info
                    None, // Destroyed
                    Default::default(),
                    AccountStatus::Destroyed,
                ),
            )]),
            contracts: Default::default(),
            reverts: Default::default(),
            state_size: 0,
            reverts_size: 0,
        };

        // Insert state should only remove the destroyed account (no code = no full clear)
        assert!(caches.insert_state(&bundle).is_ok());

        // Verify only addr1 was removed
        assert!(caches.0.account_cache.get(&addr1).is_none());
        assert!(caches.0.account_cache.get(&addr2).is_some());
    }
}
