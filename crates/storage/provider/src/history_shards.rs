//! Shared last-shard merge/rechunk for sharded history tables.
//!
//! Writers of [`tables::AccountsHistory`] and [`tables::StoragesHistory`] must:
//! 1. Group new indices by logical key.
//! 2. Finish all committed last-shard reads (and join Rayon workers) **before** opening a write
//!    batch.
//! 3. Create the `RocksDB` batch / [`EitherWriter`](crate::EitherWriter).
//! 4. Serially put the prepared physical shards.
//!
//! Parallel [`RocksDBProvider::get`](crate::providers::RocksDBProvider::get) while a write batch
//! from `with_rocksdb_batch_auto_commit` is open deadlocks for more than one unique key.

use alloy_primitives::{Address, BlockNumber, B256};
use itertools::Itertools;
use rayon::prelude::*;
use reth_db_api::{
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey, ShardedKey,
    },
    table::Table,
    tables, BlockNumberList,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::collections::BTreeMap;

/// A history table whose keys are sharded by highest block number.
///
/// The last shard for each logical key is stored with `highest_block = u64::MAX` so writers can
/// point-read it, merge new indices, and rechunk at [`NUM_OF_INDICES_IN_SHARD`].
pub trait ShardedHistoryTable: Table<Value = BlockNumberList> {
    /// Logical key that groups all shards of one history entry (address, or address+slot).
    type PartialKey: Copy + Ord + Send + Sync;

    /// Returns the logical key for a physical shard key.
    fn partial_key(key: &Self::Key) -> Self::PartialKey;

    /// Builds a physical shard key from the logical key and highest block number.
    fn shard_key(key: Self::PartialKey, highest_block: BlockNumber) -> Self::Key;

    /// Physical key of the last shard (`highest_block = u64::MAX`).
    fn last_shard_key(key: Self::PartialKey) -> Self::Key {
        Self::shard_key(key, u64::MAX)
    }
}

impl ShardedHistoryTable for tables::AccountsHistory {
    type PartialKey = Address;

    fn partial_key(key: &Self::Key) -> Self::PartialKey {
        key.key
    }

    fn shard_key(key: Self::PartialKey, highest_block: BlockNumber) -> Self::Key {
        ShardedKey::new(key, highest_block)
    }
}

impl ShardedHistoryTable for tables::StoragesHistory {
    type PartialKey = (Address, B256);

    fn partial_key(key: &Self::Key) -> Self::PartialKey {
        (key.address, key.sharded_key.key)
    }

    fn shard_key(key: Self::PartialKey, highest_block: BlockNumber) -> Self::Key {
        StorageShardedKey::new(key.0, key.1, highest_block)
    }
}

/// Physical shard puts produced by merging new indices with each key's committed last shard.
///
/// One inner vec per logical key, preserving that key's shard order. Outer order follows the
/// parallel (or serial) preparation walk and is not required to be sorted.
#[must_use = "prepared shard writes must be put"]
#[derive(Debug)]
pub struct PreparedHistoryShardWrites<T: ShardedHistoryTable> {
    per_key: Vec<Vec<(T::Key, BlockNumberList)>>,
}

impl<T: ShardedHistoryTable> PreparedHistoryShardWrites<T> {
    /// Flattened iterator of physical `(key, shard)` puts.
    pub fn into_writes(self) -> impl Iterator<Item = (T::Key, BlockNumberList)> {
        self.per_key.into_iter().flatten()
    }
}

/// Prepares history shard writes, reading last shards in parallel.
///
/// `get_last` must read **committed** state only (for example [`RocksDBProvider::get`]). Do not
/// call this while a `RocksDB` write batch is open.
pub fn prepare_history_shard_writes_parallel<T, F>(
    grouped: BTreeMap<T::PartialKey, Vec<BlockNumber>>,
    get_last: F,
) -> ProviderResult<PreparedHistoryShardWrites<T>>
where
    T: ShardedHistoryTable,
    F: Fn(T::Key) -> ProviderResult<Option<BlockNumberList>> + Send + Sync,
{
    prepare_history_shard_writes_parallel_vec(grouped.into_iter().collect(), get_last)
}

/// Prepares history shard writes from an owned vector, reading last shards in parallel.
///
/// `get_last` must read **committed** state only. Do not call this while a `RocksDB` write batch is
/// open. `grouped` must be strictly ordered by logical key so every key occurs exactly once.
pub fn prepare_history_shard_writes_parallel_vec<T, F>(
    grouped: Vec<(T::PartialKey, Vec<BlockNumber>)>,
    get_last: F,
) -> ProviderResult<PreparedHistoryShardWrites<T>>
where
    T: ShardedHistoryTable,
    F: Fn(T::Key) -> ProviderResult<Option<BlockNumberList>> + Send + Sync,
{
    validate_grouped_keys::<T>(&grouped)?;
    let per_key = grouped
        .into_par_iter()
        .with_min_len(1)
        .map(|(partial_key, indices)| prepare_one::<T, _>(partial_key, indices, &get_last))
        .collect::<ProviderResult<Vec<_>>>()?;

    Ok(PreparedHistoryShardWrites { per_key })
}

/// Prepares history shard writes, reading last shards serially.
///
/// Use this for MDBX: a `DbTx` is not safe for concurrent `get`s.
pub fn prepare_history_shard_writes_serial<T, F>(
    grouped: BTreeMap<T::PartialKey, Vec<BlockNumber>>,
    get_last: F,
) -> ProviderResult<PreparedHistoryShardWrites<T>>
where
    T: ShardedHistoryTable,
    F: FnMut(T::Key) -> ProviderResult<Option<BlockNumberList>>,
{
    prepare_history_shard_writes_serial_vec(grouped.into_iter().collect(), get_last)
}

/// Prepares history shard writes from an owned vector, reading last shards serially.
///
/// Use this for MDBX: a `DbTx` is not safe for concurrent `get`s. `grouped` must be strictly
/// ordered by logical key so every key occurs exactly once.
pub fn prepare_history_shard_writes_serial_vec<T, F>(
    grouped: Vec<(T::PartialKey, Vec<BlockNumber>)>,
    mut get_last: F,
) -> ProviderResult<PreparedHistoryShardWrites<T>>
where
    T: ShardedHistoryTable,
    F: FnMut(T::Key) -> ProviderResult<Option<BlockNumberList>>,
{
    validate_grouped_keys::<T>(&grouped)?;
    let mut per_key = Vec::with_capacity(grouped.len());
    for (partial_key, indices) in grouped {
        per_key.push(prepare_one::<T, _>(partial_key, indices, &mut get_last)?);
    }
    Ok(PreparedHistoryShardWrites { per_key })
}

fn validate_grouped_keys<T: ShardedHistoryTable>(
    grouped: &[(T::PartialKey, Vec<BlockNumber>)],
) -> ProviderResult<()> {
    if grouped.windows(2).any(|window| window[0].0 >= window[1].0) {
        return Err(ProviderError::other(InvalidGroupedHistoryKeys))
    }
    Ok(())
}

#[derive(Debug)]
struct InvalidGroupedHistoryKeys;

impl core::fmt::Display for InvalidGroupedHistoryKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("history shard preparation keys must be strictly ordered and unique")
    }
}

impl std::error::Error for InvalidGroupedHistoryKeys {}

/// Merges `indices` onto the committed last shard and rechunks at [`NUM_OF_INDICES_IN_SHARD`].
///
/// Matches `BlockNumberList::append` + last-shard `u64::MAX` key encoding used by live persist.
fn prepare_one<T, F>(
    partial_key: T::PartialKey,
    indices: Vec<BlockNumber>,
    mut get_last: F,
) -> ProviderResult<Vec<(T::Key, BlockNumberList)>>
where
    T: ShardedHistoryTable,
    F: FnMut(T::Key) -> ProviderResult<Option<BlockNumberList>>,
{
    if indices.is_empty() {
        return Ok(Vec::new());
    }

    debug_assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "indices must be strictly increasing: {indices:?}"
    );

    let last_shard_opt = get_last(T::last_shard_key(partial_key))?;
    let mut last_shard = last_shard_opt.unwrap_or_else(BlockNumberList::empty);

    last_shard.append(indices).map_err(ProviderError::other)?;

    if last_shard.len() <= NUM_OF_INDICES_IN_SHARD as u64 {
        return Ok(vec![(T::last_shard_key(partial_key), last_shard)]);
    }

    let chunks = last_shard.iter().chunks(NUM_OF_INDICES_IN_SHARD);
    let mut chunks_peekable = chunks.into_iter().peekable();
    let mut shards = Vec::new();

    while let Some(chunk) = chunks_peekable.next() {
        let shard = BlockNumberList::new_pre_sorted(chunk);
        let highest_block_number = if chunks_peekable.peek().is_some() {
            shard.iter().next_back().expect("`chunks` does not return empty list")
        } else {
            u64::MAX
        };

        shards.push((T::shard_key(partial_key, highest_block_number), shard));
    }

    Ok(shards)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
        time::Duration,
    };

    fn list(blocks: &[u64]) -> BlockNumberList {
        BlockNumberList::new(blocks.iter().copied()).unwrap()
    }

    fn blocks(shard: &BlockNumberList) -> Vec<u64> {
        shard.iter().collect()
    }

    fn account_map_getter(
        existing: HashMap<ShardedKey<Address>, BlockNumberList>,
    ) -> impl Fn(ShardedKey<Address>) -> ProviderResult<Option<BlockNumberList>> {
        move |key| Ok(existing.get(&key).cloned())
    }

    fn storage_map_getter(
        existing: HashMap<StorageShardedKey, BlockNumberList>,
    ) -> impl Fn(StorageShardedKey) -> ProviderResult<Option<BlockNumberList>> {
        move |key| Ok(existing.get(&key).cloned())
    }

    #[test]
    fn prepare_account_without_existing_shard() {
        let addr = address!("0x0000000000000000000000000000000000000001");
        let grouped = BTreeMap::from([(addr, vec![1, 2, 3])]);
        let prepared = prepare_history_shard_writes_serial::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(HashMap::new()),
        )
        .unwrap();

        let writes: Vec<_> = prepared.into_writes().collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, ShardedKey::new(addr, u64::MAX));
        assert_eq!(blocks(&writes[0].1), vec![1, 2, 3]);
    }

    #[test]
    fn prepare_account_merges_getter_last_shard() {
        let addr = address!("0x0000000000000000000000000000000000000002");
        let existing = HashMap::from([(ShardedKey::new(addr, u64::MAX), list(&[1, 2, 3]))]);
        let grouped = BTreeMap::from([(addr, vec![4, 5])]);
        let prepared = prepare_history_shard_writes_serial::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(existing),
        )
        .unwrap();

        let writes: Vec<_> = prepared.into_writes().collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, ShardedKey::new(addr, u64::MAX));
        assert_eq!(blocks(&writes[0].1), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn prepare_account_rechunks_at_shard_boundary() {
        let addr = address!("0x0000000000000000000000000000000000000003");
        let limit = NUM_OF_INDICES_IN_SHARD as u64;
        let existing_indices: Vec<u64> = (0..limit).collect();
        let existing = HashMap::from([(ShardedKey::new(addr, u64::MAX), list(&existing_indices))]);
        let grouped = BTreeMap::from([(addr, vec![limit])]);
        let prepared = prepare_history_shard_writes_serial::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(existing),
        )
        .unwrap();

        let writes: Vec<_> = prepared.into_writes().collect();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].0, ShardedKey::new(addr, limit - 1));
        assert_eq!(blocks(&writes[0].1), existing_indices);
        assert_eq!(writes[1].0, ShardedKey::new(addr, u64::MAX));
        assert_eq!(blocks(&writes[1].1), vec![limit]);
    }

    #[test]
    fn prepare_storage_without_existing_shard() {
        let addr = address!("0x0000000000000000000000000000000000000010");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");
        let grouped = BTreeMap::from([((addr, slot), vec![7, 8])]);
        let prepared = prepare_history_shard_writes_serial::<tables::StoragesHistory, _>(
            grouped,
            storage_map_getter(HashMap::new()),
        )
        .unwrap();

        let writes: Vec<_> = prepared.into_writes().collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, StorageShardedKey::new(addr, slot, u64::MAX));
        assert_eq!(blocks(&writes[0].1), vec![7, 8]);
    }

    #[test]
    fn prepare_storage_rechunks_at_shard_boundary() {
        let addr = address!("0x0000000000000000000000000000000000000011");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000002");
        let limit = NUM_OF_INDICES_IN_SHARD as u64;
        let existing_indices: Vec<u64> = (0..limit).collect();
        let existing = HashMap::from([(
            StorageShardedKey::new(addr, slot, u64::MAX),
            list(&existing_indices),
        )]);
        let grouped = BTreeMap::from([((addr, slot), vec![limit, limit + 1])]);
        let prepared = prepare_history_shard_writes_serial::<tables::StoragesHistory, _>(
            grouped,
            storage_map_getter(existing),
        )
        .unwrap();

        let writes: Vec<_> = prepared.into_writes().collect();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].0, StorageShardedKey::new(addr, slot, limit - 1));
        assert_eq!(blocks(&writes[0].1).len(), NUM_OF_INDICES_IN_SHARD);
        assert_eq!(writes[1].0, StorageShardedKey::new(addr, slot, u64::MAX));
        assert_eq!(blocks(&writes[1].1), vec![limit, limit + 1]);
    }

    #[test]
    fn prepare_skips_empty_indices() {
        let addr = address!("0x0000000000000000000000000000000000000004");
        let grouped = BTreeMap::from([(addr, Vec::new())]);
        let prepared = prepare_history_shard_writes_serial::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(HashMap::new()),
        )
        .unwrap();
        assert!(prepared.into_writes().next().is_none());
    }

    #[test]
    fn prepare_parallel_matches_serial() {
        let addr_a = address!("0x00000000000000000000000000000000000000aa");
        let addr_b = address!("0x00000000000000000000000000000000000000bb");
        let existing = HashMap::from([
            (ShardedKey::new(addr_a, u64::MAX), list(&[1, 2])),
            (ShardedKey::new(addr_b, u64::MAX), list(&[10])),
        ]);
        let grouped = BTreeMap::from([(addr_a, vec![3, 4]), (addr_b, vec![11])]);

        let serial = prepare_history_shard_writes_serial::<tables::AccountsHistory, _>(
            grouped.clone(),
            account_map_getter(existing.clone()),
        )
        .unwrap();
        let parallel = prepare_history_shard_writes_parallel::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(existing),
        )
        .unwrap();

        let mut serial_writes: Vec<_> =
            serial.into_writes().map(|(k, v)| (k, blocks(&v))).collect();
        let mut parallel_writes: Vec<_> =
            parallel.into_writes().map(|(k, v)| (k, blocks(&v))).collect();
        serial_writes.sort_by(|a, b| a.0.cmp(&b.0));
        parallel_writes.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(serial_writes, parallel_writes);
    }

    #[test]
    fn prepare_vector_parallel_matches_serial() {
        let addr_a = address!("0x00000000000000000000000000000000000000aa");
        let addr_b = address!("0x00000000000000000000000000000000000000bb");
        let existing = HashMap::from([
            (ShardedKey::new(addr_a, u64::MAX), list(&[1, 2])),
            (ShardedKey::new(addr_b, u64::MAX), list(&[10])),
        ]);
        let grouped = vec![(addr_a, vec![3, 4]), (addr_b, vec![11])];

        let serial = prepare_history_shard_writes_serial_vec::<tables::AccountsHistory, _>(
            grouped.clone(),
            account_map_getter(existing.clone()),
        )
        .unwrap();
        let parallel = prepare_history_shard_writes_parallel_vec::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(existing),
        )
        .unwrap();

        let serial_writes: Vec<_> =
            serial.into_writes().map(|(key, value)| (key, blocks(&value))).collect();
        let parallel_writes: Vec<_> =
            parallel.into_writes().map(|(key, value)| (key, blocks(&value))).collect();
        assert_eq!(serial_writes, parallel_writes);
    }

    #[test]
    fn prepare_vector_rejects_duplicate_keys() {
        let address = address!("0x00000000000000000000000000000000000000aa");
        let grouped = vec![(address, vec![1]), (address, vec![2])];

        let error = prepare_history_shard_writes_serial_vec::<tables::AccountsHistory, _>(
            grouped,
            account_map_getter(HashMap::new()),
        )
        .unwrap_err();

        assert!(error.is_other::<InvalidGroupedHistoryKeys>());
    }

    #[test]
    fn prepare_parallel_overlaps_gets_on_dedicated_pool() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let inflight = AtomicUsize::new(0);
        let max_inflight = AtomicUsize::new(0);

        let grouped: Vec<_> =
            (1u8..=8).map(|i| (Address::repeat_byte(i), vec![u64::from(i)])).collect();

        let prepared = pool
            .install(|| {
                prepare_history_shard_writes_parallel_vec::<tables::AccountsHistory, _>(
                    grouped,
                    |_| {
                        let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_inflight.fetch_max(current, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(50));
                        inflight.fetch_sub(1, Ordering::SeqCst);
                        Ok(None)
                    },
                )
            })
            .unwrap();
        assert_eq!(prepared.into_writes().count(), 8);

        assert!(
            max_inflight.load(Ordering::SeqCst) >= 2,
            "parallel last-shard gets should overlap on a 4-thread pool, max inflight was {}",
            max_inflight.load(Ordering::SeqCst)
        );
    }
}
