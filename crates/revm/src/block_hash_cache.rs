//! Helpers for warming block-hash caches during payload building.

use crate::cached::CachedReads;
use alloy_primitives::{BlockNumber, B256};
use reth_storage_api::BlockHashReader;
use reth_storage_errors::provider::ProviderResult;
use revm::{
    database::{states::block_hash_cache::BlockHashCache, State},
    primitives::BLOCK_HASH_HISTORY,
};

/// Returns the block numbers accessible to `BLOCKHASH` when executing `block_number`.
///
/// The range is `[start, block_number)` — up to 256 blocks before the block being executed.
#[inline]
pub const fn block_hash_window(block_number: BlockNumber) -> (BlockNumber, BlockNumber) {
    let start = block_number.saturating_sub(BLOCK_HASH_HISTORY);
    (start, block_number)
}

/// Returns `true` if every block number in the window is present in `cached_reads`.
fn is_window_cached(cached_reads: &CachedReads, start: BlockNumber, end: BlockNumber) -> bool {
    (start..end).all(|number| cached_reads.block_hashes.contains_key(&number))
}

/// Builds a [`BlockHashCache`] from entries in `cached_reads` for the given window.
fn block_hash_cache_from_reads(
    cached_reads: &CachedReads,
    start: BlockNumber,
    end: BlockNumber,
) -> BlockHashCache {
    let mut cache = BlockHashCache::new();
    for number in start..end {
        if let Some(&hash) = cached_reads.block_hashes.get(&number) {
            cache.insert(number, hash);
        }
    }
    cache
}

/// Fetches missing block hashes in `start..end` and inserts them into `cached_reads`.
fn fill_missing_block_hashes(
    reader: &impl BlockHashReader,
    cached_reads: &mut CachedReads,
    start: BlockNumber,
    end: BlockNumber,
) -> ProviderResult<()> {
    let expected_len = (end - start) as usize;
    let bulk = reader.canonical_hashes_range(start, end)?;

    if bulk.len() == expected_len {
        for (offset, hash) in bulk.iter().enumerate() {
            let number = start + offset as u64;
            cached_reads.block_hashes.insert(number, *hash);
        }
        return Ok(())
    }

    for number in start..end {
        if cached_reads.block_hashes.contains_key(&number) {
            continue
        }
        if let Some(hash) = reader.block_hash(number)? {
            cached_reads.block_hashes.insert(number, hash);
        }
    }

    Ok(())
}

/// Warms `cached_reads` and returns a [`BlockHashCache`] for [`StateBuilder::with_block_hashes`].
///
/// If the window is already present in `cached_reads`, no database access is performed.
///
/// [`StateBuilder::with_block_hashes`]: revm::database::StateBuilder::with_block_hashes
pub fn warm_block_hash_cache(
    reader: &impl BlockHashReader,
    block_number: BlockNumber,
    cached_reads: &mut CachedReads,
) -> ProviderResult<BlockHashCache> {
    let (start, end) = block_hash_window(block_number);

    if !is_window_cached(cached_reads, start, end) {
        fill_missing_block_hashes(reader, cached_reads, start, end)?;
    }

    Ok(block_hash_cache_from_reads(cached_reads, start, end))
}

/// Collects block hashes from `State`.
///
/// Used to sync hashes back into [`CachedReads`] after the [`State`] has been dropped, since
/// `State` holds a mutable borrow of `CachedReads` through its database wrapper.
pub fn collect_block_hashes_from_state<DB>(state: &State<DB>) -> alloc::vec::Vec<(u64, B256)> {
    state.block_hashes.iter().collect()
}

/// Merges collected block hashes into `cached_reads`.
pub fn extend_cached_block_hashes(
    cached_reads: &mut CachedReads,
    hashes: impl IntoIterator<Item = (u64, B256)>,
) {
    for (number, hash) in hashes {
        cached_reads.block_hashes.insert(number, hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::StateProviderTest;
    use alloy_primitives::{keccak256, B256, U256};

    fn insert_hashes(provider: &mut StateProviderTest, start: u64, end: u64) {
        for number in start..end {
            provider.insert_block_hash(number, keccak256(U256::from(number).to_be_bytes::<32>()));
        }
    }

    #[test]
    fn block_hash_window_returns_256_block_range() {
        assert_eq!(block_hash_window(300), (44, 300));
        assert_eq!(block_hash_window(100), (0, 100));
    }

    #[test]
    fn warm_block_hash_cache_populates_reads_and_cache() {
        let mut provider = StateProviderTest::default();
        insert_hashes(&mut provider, 44, 300);

        let mut cached_reads = CachedReads::default();
        let cache = warm_block_hash_cache(&provider, 300, &mut cached_reads).unwrap();

        assert_eq!(cached_reads.block_hashes.len(), 256);
        assert_eq!(cache.get(44), cached_reads.block_hashes.get(&44).copied());
        assert_eq!(cache.get(299), cached_reads.block_hashes.get(&299).copied());
    }

    #[test]
    fn warm_block_hash_cache_skips_reader_when_window_is_cached() {
        let mut provider = StateProviderTest::default();
        insert_hashes(&mut provider, 0, 10);

        let mut cached_reads = CachedReads::default();
        warm_block_hash_cache(&provider, 10, &mut cached_reads).unwrap();

        let provider = StateProviderTest::default();
        warm_block_hash_cache(&provider, 10, &mut cached_reads).unwrap();

        assert_eq!(cached_reads.block_hashes.len(), 10);
    }

    #[test]
    fn extend_cached_block_hashes_merges_collected_hashes() {
        let mut state = State::builder().build();
        let hash = B256::repeat_byte(0xab);
        state.block_hashes.insert(42, hash);

        let collected = collect_block_hashes_from_state(&state);
        let mut cached_reads = CachedReads::default();
        extend_cached_block_hashes(&mut cached_reads, collected);

        assert_eq!(cached_reads.block_hashes.get(&42), Some(&hash));
    }
}
