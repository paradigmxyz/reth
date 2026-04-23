//! Parallel pre-block snapshot build.
//!
//! Populates a `BlockPreState` by fetching every key listed in `RequiredReads`, plus the
//! recent-256 block hashes for `BLOCKHASH`. Reads route through a per-thread prewarming
//! [`CachedStateProvider`], so cache misses are served from the underlying provider and written
//! back in one pass — this is the sole cache-writer window per block (see `BAL.md`
//! §`ExecutionCache` interactions).
//!
//! ### Per-thread provider construction
//!
//! `StateProviderBox` is `Box<dyn StateProvider + Send>` (no `Sync`), so it can't be Arc-shared
//! across rayon workers. Instead we take a [`StateProviderBuilder`] (which IS `Send + Sync`
//! through its Arc'd factory) and have each rayon worker lazily build its own
//! `CachedStateProvider<StateProviderBox, true>` via [`rayon::iter::ParallelIterator::map_init`].
//! Same pattern that `prefetch_bal_storage` uses in `prewarm.rs:740`.
//!
//! A provider error in any phase aborts the whole build: unlike the opportunistic `prewarm.rs`
//! prefetch, a missing snapshot entry is unrecoverable here (workers would surface it as
//! `BalError::AccountNotFound`).
//!
//! Structured in four phases:
//! 1. 1a — parallel account fetch, 1b — parallel storage fetch (run concurrently).
//! 2. Collect `code_hash` values from phase-1a output.
//! 3. Parallel bytecode fetch keyed by `code_hash`.
//! 4. Serial `canonical_hashes_range` for the `BLOCKHASH` window.

use super::pre_state::{BlockPreState, RequiredReads};
use crate::tree::{CachedStateMetrics, CachedStateProvider, SavedCache, StateProviderBuilder};
use alloy_primitives::{map::B256HashMap, Address, BlockNumber, StorageKey, StorageValue, B256};
use rayon::prelude::*;
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode, NodePrimitives};
use reth_provider::{
    AccountReader, BlockHashReader, BlockReader, BytecodeReader, StateProvider, StateProviderBox,
    StateProviderFactory, StateReader,
};
use std::collections::HashMap;

/// Size of the recent-block window exposed by the `BLOCKHASH` opcode.
const BLOCK_HASH_WINDOW: u64 = 256;

/// Per-thread prewarming-cached provider. Built lazily by each rayon worker on first use via
/// `map_init`, then reused for every item that worker processes.
type ThreadCachedProvider = CachedStateProvider<StateProviderBox, true>;

/// Builds the pre-block snapshot in parallel.
///
/// `provider_builder` must produce a `Send`-able `StateProviderBox` per call to `.build()`. Each
/// rayon worker calls it once and wraps the result in a thread-local prewarming
/// `CachedStateProvider` so cache misses populate `saved_cache` for cross-block warmth.
///
/// Fails fast on the first [`reth_errors::ProviderError`]. The caller (engine tree) is
/// responsible for ensuring the passed `saved_cache` is the exclusive cache handle for this
/// block's execution (`BAL.md` §`ExecutionCache`: "snapshot build is the only writer to
/// `ExecutionCache` during a block's execution").
pub fn build_pre_state<N, P>(
    provider_builder: StateProviderBuilder<N, P>,
    saved_cache: SavedCache,
    cache_metrics: CachedStateMetrics,
    reads: &RequiredReads,
    block_number: BlockNumber,
) -> ProviderResult<BlockPreState>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    // Phases 1a + 1b: accounts and storage have no cross-dependency; run concurrently.
    let (accounts_result, storage_result) = rayon::join(
        || {
            fetch_accounts(
                &provider_builder,
                &saved_cache,
                &cache_metrics,
                reads.addresses.iter().copied(),
            )
        },
        || {
            fetch_storage(
                &provider_builder,
                &saved_cache,
                &cache_metrics,
                reads.storage.iter().copied(),
            )
        },
    );
    let accounts = accounts_result?;
    let storage = storage_result?;

    // Phases 2 + 3: derive code_hashes from resolved accounts, then fetch bytecode in parallel.
    let code = fetch_code(&provider_builder, &saved_cache, &cache_metrics, &accounts)?;

    // Phase 4: BLOCKHASH window. Single provider on the calling thread — the slice is small
    // (≤256 entries) and a single `canonical_hashes_range` call covers it.
    let block_hashes = fetch_block_hashes(&provider_builder, block_number)?;

    Ok(BlockPreState { accounts, storage, code, block_hashes })
}

/// Builds a fresh per-thread provider wrapped in the prewarming cache. Returns
/// `ProviderResult` so an init failure on one thread propagates as the per-item error to every
/// item that thread would have handled.
fn init_thread_provider<N, P>(
    builder: &StateProviderBuilder<N, P>,
    saved_cache: &SavedCache,
    cache_metrics: &CachedStateMetrics,
) -> ProviderResult<ThreadCachedProvider>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let built = builder.build()?;
    Ok(CachedStateProvider::new_prewarm(built, saved_cache.cache().clone(), cache_metrics.clone()))
}

/// Parallel fetch of every declared address's basic account.
pub(super) fn fetch_accounts<N, P>(
    builder: &StateProviderBuilder<N, P>,
    saved_cache: &SavedCache,
    cache_metrics: &CachedStateMetrics,
    addresses: impl IntoIterator<Item = Address>,
) -> ProviderResult<HashMap<Address, Option<Account>>>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let addrs: Vec<Address> = addresses.into_iter().collect();
    addrs
        .into_par_iter()
        .map_init(
            || init_thread_provider(builder, saved_cache, cache_metrics),
            |provider_result, addr| {
                let provider = provider_result.as_ref().map_err(Clone::clone)?;
                provider.basic_account(&addr).map(|opt| (addr, opt))
            },
        )
        .collect()
}

/// Parallel fetch of every `(address, slot)` pair's pre-block storage value. Missing entries
/// (provider returns `Ok(None)`) are recorded as `StorageValue::ZERO`.
pub(super) fn fetch_storage<N, P>(
    builder: &StateProviderBuilder<N, P>,
    saved_cache: &SavedCache,
    cache_metrics: &CachedStateMetrics,
    keys: impl IntoIterator<Item = (Address, StorageKey)>,
) -> ProviderResult<HashMap<(Address, StorageKey), StorageValue>>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let keys: Vec<(Address, StorageKey)> = keys.into_iter().collect();
    keys.into_par_iter()
        .map_init(
            || init_thread_provider(builder, saved_cache, cache_metrics),
            |provider_result, (addr, slot)| {
                let provider = provider_result.as_ref().map_err(Clone::clone)?;
                provider.storage(addr, slot).map(|opt| ((addr, slot), opt.unwrap_or_default()))
            },
        )
        .collect()
}

/// Parallel fetch of every distinct bytecode referenced by the resolved accounts.
pub(super) fn fetch_code<N, P>(
    builder: &StateProviderBuilder<N, P>,
    saved_cache: &SavedCache,
    cache_metrics: &CachedStateMetrics,
    accounts: &HashMap<Address, Option<Account>>,
) -> ProviderResult<B256HashMap<Option<Bytecode>>>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    // Dedupe code hashes across accounts so we fetch each bytecode once even if multiple
    // accounts share the same code (common for proxy / clone patterns).
    let hashes: Vec<B256> = accounts
        .values()
        .filter_map(|opt| opt.as_ref().and_then(|a| a.bytecode_hash))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let pairs: Vec<(B256, Option<Bytecode>)> = hashes
        .into_par_iter()
        .map_init(
            || init_thread_provider(builder, saved_cache, cache_metrics),
            |provider_result, hash| {
                let provider = provider_result.as_ref().map_err(Clone::clone)?;
                provider.bytecode_by_hash(&hash).map(|code| (hash, code))
            },
        )
        .collect::<ProviderResult<_>>()?;

    Ok(pairs.into_iter().collect())
}

/// Fetches `[block_number - 256, block_number)` canonical hashes on the main thread.
///
/// At genesis (`block_number < 256`) the window shrinks naturally via `saturating_sub`. Builds
/// a single provider via `builder.build()` — no parallelism needed for ≤256 entries fetched in
/// one `canonical_hashes_range` call.
pub(super) fn fetch_block_hashes<N, P>(
    builder: &StateProviderBuilder<N, P>,
    block_number: BlockNumber,
) -> ProviderResult<HashMap<u64, B256>>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let provider = builder.build()?;
    let start = block_number.saturating_sub(BLOCK_HASH_WINDOW);
    let hashes = provider.canonical_hashes_range(start, block_number)?;
    Ok(hashes.into_iter().enumerate().map(|(i, hash)| (start + i as u64, hash)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::ExecutionCache;
    use alloy_primitives::{keccak256, Bytes, U256};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use std::collections::BTreeSet;

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn slot_key(byte: u8) -> StorageKey {
        let mut b = [0u8; 32];
        b[31] = byte;
        B256::from(b)
    }

    fn test_cache() -> (SavedCache, CachedStateMetrics) {
        (SavedCache::new(B256::ZERO, ExecutionCache::new(1 << 20)), CachedStateMetrics::default())
    }

    /// Wraps a `MockEthProvider` in a `StateProviderBuilder` so the snapshot build can use
    /// `.build()` per worker (see module-level docs on per-thread provider construction).
    fn builder_for(
        provider: MockEthProvider,
    ) -> StateProviderBuilder<reth_ethereum_primitives::EthPrimitives, MockEthProvider> {
        StateProviderBuilder::new(provider, B256::ZERO, None)
    }

    #[test]
    fn happy_path_populates_all_maps() {
        let tp: MockEthProvider = MockEthProvider::new();
        let bytecode_bytes = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = keccak256(&bytecode_bytes);

        tp.add_account(
            addr(1),
            ExtendedAccount::new(1, U256::from(100))
                .with_bytecode(bytecode_bytes)
                .extend_storage([(slot_key(10), U256::from(42))]),
        );

        let reads = RequiredReads {
            addresses: BTreeSet::from([addr(1)]),
            storage: BTreeSet::from([(addr(1), slot_key(10))]),
        };

        let (cache, metrics) = test_cache();
        let pre =
            build_pre_state(builder_for(tp), cache, metrics, &reads, 0).expect("build succeeds");

        let acc = pre.accounts.get(&addr(1)).unwrap().as_ref().unwrap();
        assert_eq!(acc.nonce, 1);
        assert_eq!(acc.balance, U256::from(100));
        assert_eq!(acc.bytecode_hash, Some(code_hash));
        assert_eq!(pre.storage.get(&(addr(1), slot_key(10))).copied(), Some(U256::from(42)));
        assert!(pre.code.contains_key(&code_hash));
    }

    #[test]
    fn missing_account_produces_none_entry() {
        let tp: MockEthProvider = MockEthProvider::new();
        let reads = RequiredReads { addresses: BTreeSet::from([addr(99)]), ..Default::default() };
        let (cache, metrics) = test_cache();
        let pre = build_pre_state(builder_for(tp), cache, metrics, &reads, 0).unwrap();

        // Present in map, but with a None value: "account doesn't exist pre-block".
        assert_eq!(pre.accounts.get(&addr(99)), Some(&None));
    }

    #[test]
    fn missing_storage_becomes_zero() {
        let tp: MockEthProvider = MockEthProvider::new();
        let reads = RequiredReads {
            storage: BTreeSet::from([(addr(1), slot_key(10))]),
            ..Default::default()
        };
        let (cache, metrics) = test_cache();
        let pre = build_pre_state(builder_for(tp), cache, metrics, &reads, 0).unwrap();

        assert_eq!(pre.storage.get(&(addr(1), slot_key(10))).copied(), Some(U256::ZERO));
    }

    #[test]
    fn eoa_account_produces_no_code_entry() {
        let tp: MockEthProvider = MockEthProvider::new();
        tp.add_account(addr(1), ExtendedAccount::new(0, U256::from(1)));

        let reads = RequiredReads { addresses: BTreeSet::from([addr(1)]), ..Default::default() };
        let (cache, metrics) = test_cache();
        let pre = build_pre_state(builder_for(tp), cache, metrics, &reads, 0).unwrap();

        assert!(pre.code.is_empty());
    }

    #[test]
    fn duplicate_code_hash_fetched_once() {
        // Two accounts sharing bytecode → one code entry keyed by hash.
        let tp: MockEthProvider = MockEthProvider::new();
        let bytecode_bytes = Bytes::from_static(&[0xff]);
        let code_hash = keccak256(&bytecode_bytes);

        tp.add_account(
            addr(1),
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytecode_bytes.clone()),
        );
        tp.add_account(addr(2), ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytecode_bytes));

        let reads =
            RequiredReads { addresses: BTreeSet::from([addr(1), addr(2)]), ..Default::default() };
        let (cache, metrics) = test_cache();
        let pre = build_pre_state(builder_for(tp), cache, metrics, &reads, 0).unwrap();

        assert_eq!(pre.code.len(), 1);
        assert!(pre.code.contains_key(&code_hash));
    }

    #[test]
    fn empty_reads_still_returns_ok() {
        let tp: MockEthProvider = MockEthProvider::new();
        let (cache, metrics) = test_cache();
        let pre =
            build_pre_state(builder_for(tp), cache, metrics, &RequiredReads::default(), 0).unwrap();

        assert!(pre.accounts.is_empty());
        assert!(pre.storage.is_empty());
        assert!(pre.code.is_empty());
        // block_hashes: MockEthProvider returns empty Vec for an uninitialized range — fine.
        assert!(pre.block_hashes.is_empty());
    }
}
