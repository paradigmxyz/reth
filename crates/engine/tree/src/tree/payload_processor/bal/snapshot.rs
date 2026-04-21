//! Parallel pre-block snapshot build.
//!
//! Populates a `BlockPreState` by fetching every key listed in `RequiredReads`, plus the
//! recent-256 block hashes for `BLOCKHASH`. Reads route through a prewarming
//! [`CachedStateProvider`], so cache misses are served from the underlying provider and written
//! back in one pass — this is the sole cache-writer window per block (see `BAL.md`
//! §`ExecutionCache` interactions).
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
use alloy_primitives::{map::B256HashMap, Address, BlockNumber, StorageKey, StorageValue, B256};
use rayon::prelude::*;
use reth_errors::ProviderResult;
use reth_execution_cache::{CachedStateMetrics, CachedStateProvider, ExecutionCache};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{AccountReader, BlockHashReader, BytecodeReader, StateProvider};
use std::{collections::HashMap, sync::Arc};

/// Size of the recent-block window exposed by the `BLOCKHASH` opcode.
const BLOCK_HASH_WINDOW: u64 = 256;

/// Builds the pre-block snapshot in parallel.
///
/// `provider` must be `Send + Sync`; every phase shares a single `CachedStateProvider` wrapper
/// over it, which is what writes through to the `ExecutionCache` on miss.
///
/// Fails fast on the first [`reth_errors::ProviderError`]. The caller (engine tree) is
/// responsible for ensuring the passed `cache` is the exclusive cache handle for this block's
/// execution (`BAL.md` §`ExecutionCache`: "snapshot build is the only writer to
/// `ExecutionCache` during a block's execution").
pub fn build_pre_state<P>(
    provider: Arc<P>,
    cache: ExecutionCache,
    metrics: CachedStateMetrics,
    reads: &RequiredReads,
    block_number: BlockNumber,
) -> ProviderResult<BlockPreState>
where
    P: StateProvider + Send + Sync + 'static,
{
    let cached: Arc<CachedStateProvider<Arc<P>, true>> =
        Arc::new(CachedStateProvider::new_prewarm(provider.clone(), cache, metrics));

    // Phases 1a + 1b: accounts and storage have no cross-dependency; run concurrently.
    let (accounts_result, storage_result) = rayon::join(
        || fetch_accounts(cached.as_ref(), reads.addresses.iter().copied()),
        || fetch_storage(cached.as_ref(), reads.storage.iter().copied()),
    );
    let accounts = accounts_result?;
    let storage = storage_result?;

    // Phases 2 + 3: derive code_hashes from resolved accounts, then fetch bytecode in parallel.
    let code = fetch_code(cached.as_ref(), &accounts)?;

    // Phase 4: BLOCKHASH window. Fetched from the raw provider to avoid adding unrelated entries
    // to the caches.
    let block_hashes = fetch_block_hashes(provider.as_ref(), block_number)?;

    Ok(BlockPreState { accounts, storage, code, block_hashes })
}

/// Parallel fetch of every declared address's basic account.
pub(super) fn fetch_accounts<P>(
    provider: &P,
    addresses: impl IntoIterator<Item = Address>,
) -> ProviderResult<HashMap<Address, Option<Account>>>
where
    P: AccountReader + Sync,
{
    let addrs: Vec<Address> = addresses.into_iter().collect();
    addrs.into_par_iter().map(|addr| provider.basic_account(&addr).map(|opt| (addr, opt))).collect()
}

/// Parallel fetch of every `(address, slot)` pair's pre-block storage value. Missing entries
/// (provider returns `Ok(None)`) are recorded as `StorageValue::ZERO`.
pub(super) fn fetch_storage<P>(
    provider: &P,
    keys: impl IntoIterator<Item = (Address, StorageKey)>,
) -> ProviderResult<HashMap<(Address, StorageKey), StorageValue>>
where
    P: StateProvider + Sync,
{
    let keys: Vec<(Address, StorageKey)> = keys.into_iter().collect();
    keys.into_par_iter()
        .map(|(addr, slot)| {
            provider.storage(addr, slot).map(|opt| ((addr, slot), opt.unwrap_or_default()))
        })
        .collect()
}

/// Parallel fetch of every distinct bytecode referenced by the resolved accounts.
pub(super) fn fetch_code<P>(
    provider: &P,
    accounts: &HashMap<Address, Option<Account>>,
) -> ProviderResult<B256HashMap<Option<Bytecode>>>
where
    P: BytecodeReader + Sync,
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
        .map(|hash| provider.bytecode_by_hash(&hash).map(|code| (hash, code)))
        .collect::<ProviderResult<_>>()?;

    Ok(pairs.into_iter().collect())
}

/// Fetches `[block_number - 256, block_number)` canonical hashes on the main thread.
///
/// At genesis (`block_number < 256`) the window shrinks naturally via `saturating_sub`.
pub(super) fn fetch_block_hashes<P>(
    provider: &P,
    block_number: BlockNumber,
) -> ProviderResult<HashMap<u64, B256>>
where
    P: BlockHashReader,
{
    let start = block_number.saturating_sub(BLOCK_HASH_WINDOW);
    let hashes = provider.canonical_hashes_range(start, block_number)?;
    Ok(hashes.into_iter().enumerate().map(|(i, hash)| (start + i as u64, hash)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn test_cache() -> (ExecutionCache, CachedStateMetrics) {
        (ExecutionCache::new(1 << 20), CachedStateMetrics::default())
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
        let pre = build_pre_state(Arc::new(tp), cache, metrics, &reads, 0).expect("build succeeds");

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
        let pre = build_pre_state(Arc::new(tp), cache, metrics, &reads, 0).unwrap();

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
        let pre = build_pre_state(Arc::new(tp), cache, metrics, &reads, 0).unwrap();

        assert_eq!(pre.storage.get(&(addr(1), slot_key(10))).copied(), Some(U256::ZERO));
    }

    #[test]
    fn eoa_account_produces_no_code_entry() {
        let tp: MockEthProvider = MockEthProvider::new();
        tp.add_account(addr(1), ExtendedAccount::new(0, U256::from(1)));

        let reads = RequiredReads { addresses: BTreeSet::from([addr(1)]), ..Default::default() };
        let (cache, metrics) = test_cache();
        let pre = build_pre_state(Arc::new(tp), cache, metrics, &reads, 0).unwrap();

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
        let pre = build_pre_state(Arc::new(tp), cache, metrics, &reads, 0).unwrap();

        assert_eq!(pre.code.len(), 1);
        assert!(pre.code.contains_key(&code_hash));
    }

    #[test]
    fn empty_reads_still_returns_ok() {
        let tp: MockEthProvider = MockEthProvider::new();
        let (cache, metrics) = test_cache();
        let pre =
            build_pre_state(Arc::new(tp), cache, metrics, &RequiredReads::default(), 0).unwrap();

        assert!(pre.accounts.is_empty());
        assert!(pre.storage.is_empty());
        assert!(pre.code.is_empty());
        // block_hashes: MockEthProvider returns empty Vec for an uninitialized range — fine.
        assert!(pre.block_hashes.is_empty());
    }
}
