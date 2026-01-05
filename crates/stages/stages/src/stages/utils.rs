//! Utils for `stages`.
use alloy_primitives::{BlockNumber, TxNumber};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::sharded_key::NUM_OF_INDICES_IN_SHARD,
    table::{Decompress, Table},
    transaction::{DbTx, DbTxMut},
    BlockNumberList, DatabaseError,
};
use reth_etl::Collector;
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::providers::{RocksDBBatch, RocksDBProvider};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::EitherWriter;
use reth_provider::{
    providers::StaticFileProvider, BlockReader, DBProvider, ProviderError,
    StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use std::{collections::HashMap, hash::Hash, ops::RangeBounds};
use tracing::info;

/// Number of blocks before pushing indices from cache to [`Collector`]
const DEFAULT_CACHE_THRESHOLD: u64 = 100_000;

/// Collects all history (`H`) indices for a range of changesets (`CS`) and stores them in a
/// [`Collector`].
///
/// ## Process
/// The function utilizes a `HashMap` cache with a structure of `PartialKey` (`P`) (Address or
/// Address.StorageKey) to `BlockNumberList`. When the cache exceeds its capacity, its contents are
/// moved to a [`Collector`]. Here, each entry's key is a concatenation of `PartialKey` and the
/// highest block number in its list.
///
/// ## Example
/// 1. Initial Cache State: `{ Address1: [1,2,3], ... }`
/// 2. Cache is flushed to the `Collector`.
/// 3. Updated Cache State: `{ Address1: [100,300], ... }`
/// 4. Cache is flushed again.
///
/// As a result, the `Collector` will contain entries such as `(Address1.3, [1,2,3])` and
/// `(Address1.300, [100,300])`. The entries may be stored across one or more files.
pub(crate) fn collect_history_indices<Provider, CS, H, P>(
    provider: &Provider,
    range: impl RangeBounds<CS::Key>,
    sharded_key_factory: impl Fn(P, BlockNumber) -> H::Key,
    partial_key_factory: impl Fn((CS::Key, CS::Value)) -> (u64, P),
    etl_config: &EtlConfig,
) -> Result<Collector<H::Key, H::Value>, StageError>
where
    Provider: DBProvider,
    CS: Table,
    H: Table<Value = BlockNumberList>,
    P: Copy + Eq + Hash,
{
    let mut changeset_cursor = provider.tx_ref().cursor_read::<CS>()?;

    let mut collector = Collector::new(etl_config.file_size, etl_config.dir.clone());
    let mut cache: HashMap<P, Vec<u64>> = HashMap::default();

    let mut collect = |cache: &HashMap<P, Vec<u64>>| {
        for (key, indices) in cache {
            let last = indices.last().expect("qed");
            collector.insert(
                sharded_key_factory(*key, *last),
                BlockNumberList::new_pre_sorted(indices.iter().copied()),
            )?;
        }
        Ok::<(), StageError>(())
    };

    // observability
    let total_changesets = provider.tx_ref().entries::<CS>()?;
    let interval = (total_changesets / 1000).max(1);

    let mut flush_counter = 0;
    let mut current_block_number = u64::MAX;
    for (idx, entry) in changeset_cursor.walk_range(range)?.enumerate() {
        let (block_number, key) = partial_key_factory(entry?);
        cache.entry(key).or_default().push(block_number);

        if idx > 0 && idx.is_multiple_of(interval) && total_changesets > 1000 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.4}%", (idx as f64 / total_changesets as f64) * 100.0), "Collecting indices");
        }

        // Make sure we only flush the cache every DEFAULT_CACHE_THRESHOLD blocks.
        if current_block_number != block_number {
            current_block_number = block_number;
            flush_counter += 1;
            if flush_counter > DEFAULT_CACHE_THRESHOLD {
                collect(&cache)?;
                cache.clear();
                flush_counter = 0;
            }
        }
    }
    collect(&cache)?;

    Ok(collector)
}

/// Given a [`Collector`] created by [`collect_history_indices`] it iterates all entries, loading
/// the indices into the database in shards.
///
///  ## Process
/// Iterates over elements, grouping indices by their partial keys (e.g., `Address` or
/// `Address.StorageKey`). It flushes indices to disk when reaching a shard's max length
/// (`NUM_OF_INDICES_IN_SHARD`) or when the partial key changes, ensuring the last previous partial
/// key shard is stored.
pub(crate) fn load_history_indices<Provider, H, P>(
    provider: &Provider,
    mut collector: Collector<H::Key, H::Value>,
    append_only: bool,
    sharded_key_factory: impl Clone + Fn(P, u64) -> <H as Table>::Key,
    decode_key: impl Fn(Vec<u8>) -> Result<<H as Table>::Key, DatabaseError>,
    get_partial: impl Fn(<H as Table>::Key) -> P,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut>,
    H: Table<Value = BlockNumberList>,
    P: Copy + Default + Eq,
{
    let mut write_cursor = provider.tx_ref().cursor_write::<H>()?;
    let mut current_partial = P::default();
    let mut current_list = Vec::<u64>::new();

    // observability
    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = decode_key(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        // AccountsHistory: `Address`.
        // StorageHistory: `Address.StorageKey`.
        let partial_key = get_partial(sharded_key);

        if current_partial != partial_key {
            // We have reached the end of this subset of keys so
            // we need to flush its last indice shard.
            load_indices(
                &mut write_cursor,
                current_partial,
                &mut current_list,
                &sharded_key_factory,
                append_only,
                LoadMode::Flush,
            )?;

            current_partial = partial_key;
            current_list.clear();

            // If it's not the first sync, there might an existing shard already, so we need to
            // merge it with the one coming from the collector
            if !append_only &&
                let Some((_, last_database_shard)) =
                    write_cursor.seek_exact(sharded_key_factory(current_partial, u64::MAX))?
            {
                current_list.extend(last_database_shard.iter());
            }
        }

        current_list.extend(new_list.iter());
        load_indices(
            &mut write_cursor,
            current_partial,
            &mut current_list,
            &sharded_key_factory,
            append_only,
            LoadMode::KeepLast,
        )?;
    }

    // There will be one remaining shard that needs to be flushed to DB.
    load_indices(
        &mut write_cursor,
        current_partial,
        &mut current_list,
        &sharded_key_factory,
        append_only,
        LoadMode::Flush,
    )?;

    Ok(())
}

/// Shard and insert the indices list according to [`LoadMode`] and its length.
pub(crate) fn load_indices<H, C, P>(
    cursor: &mut C,
    partial_key: P,
    list: &mut Vec<BlockNumber>,
    sharded_key_factory: &impl Fn(P, BlockNumber) -> <H as Table>::Key,
    append_only: bool,
    mode: LoadMode,
) -> Result<(), StageError>
where
    C: DbCursorRO<H> + DbCursorRW<H>,
    H: Table<Value = BlockNumberList>,
    P: Copy,
{
    if list.len() > NUM_OF_INDICES_IN_SHARD || mode.is_flush() {
        let chunks = list
            .chunks(NUM_OF_INDICES_IN_SHARD)
            .map(|chunks| chunks.to_vec())
            .collect::<Vec<Vec<u64>>>();

        let mut iter = chunks.into_iter().peekable();
        while let Some(chunk) = iter.next() {
            let mut highest = *chunk.last().expect("at least one index");

            if !mode.is_flush() && iter.peek().is_none() {
                *list = chunk;
            } else {
                if iter.peek().is_none() {
                    highest = u64::MAX;
                }
                let key = sharded_key_factory(partial_key, highest);
                let value = BlockNumberList::new_pre_sorted(chunk);

                if append_only {
                    cursor.append(key, &value)?;
                } else {
                    cursor.upsert(key, &value)?;
                }
            }
        }
    }

    Ok(())
}

/// Mode on how to load index shards into the database.
pub(crate) enum LoadMode {
    /// Keep the last shard in memory and don't flush it to the database.
    KeepLast,
    /// Flush all shards into the database.
    Flush,
}

impl LoadMode {
    const fn is_flush(&self) -> bool {
        matches!(self, Self::Flush)
    }
}

/// Deletes all entries in a `RocksDB` table by enqueuing delete operations in the provided batch.
#[cfg(all(unix, feature = "rocksdb"))]
pub(crate) fn clear_rocksdb_table<T: Table>(
    rocksdb: &RocksDBProvider,
    batch: &mut RocksDBBatch<'_>,
) -> Result<(), StageError> {
    for entry in rocksdb.iter::<T>()? {
        let (key, _value) = entry?;
        batch.delete::<T>(key)?;
    }
    Ok(())
}

/// Load storage history indices through an [`EitherWriter`] which routes writes to either
/// database or `RocksDB`.
///
/// This is similar to [`load_history_indices`] but uses `EitherWriter` for flexible storage
/// backend routing. The sharding logic is identical: indices are grouped by partial keys,
/// shards are created when reaching `NUM_OF_INDICES_IN_SHARD`, and the last shard uses
/// `u64::MAX` as the highest block number sentinel.
///
/// Note: For `RocksDB`, when `append_only == false`, this function reads existing shards
/// from `RocksDB` and merges them with new indices.
#[cfg(all(unix, feature = "rocksdb"))]
pub(crate) fn load_storage_history_indices_via_writer<CURSOR, N, Provider>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    mut collector: reth_etl::Collector<
        reth_db_api::models::storage_sharded_key::StorageShardedKey,
        BlockNumberList,
    >,
    append_only: bool,
    rocksdb_provider: &Provider,
) -> Result<(), StageError>
where
    CURSOR: DbCursorRW<reth_db_api::tables::StoragesHistory>
        + DbCursorRO<reth_db_api::tables::StoragesHistory>,
    N: reth_primitives_traits::NodePrimitives,
    Provider: reth_provider::RocksDBProviderFactory,
{
    use reth_db_api::{models::storage_sharded_key::StorageShardedKey, table::Decode};

    type PartialKey = (alloy_primitives::Address, alloy_primitives::B256);

    let mut current_partial = PartialKey::default();
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    let rocksdb = rocksdb_provider.rocksdb_provider();

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = StorageShardedKey::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_storage_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices via EitherWriter");
        }

        let partial_key: PartialKey = (sharded_key.address, sharded_key.sharded_key.key);

        if current_partial != partial_key {
            flush_storage_shards(writer, current_partial, &mut current_list, append_only, true)?;
            current_partial = partial_key;
            current_list.clear();

            // If it's not the first sync, there might be an existing shard already in RocksDB,
            // so we need to merge it with the one coming from the collector
            if !append_only {
                let key = StorageShardedKey::new(partial_key.0, partial_key.1, u64::MAX);
                if let Some(existing_list) =
                    rocksdb.get::<reth_db_api::tables::StoragesHistory>(key)?
                {
                    current_list.extend(existing_list.iter());
                }
            }
        }

        current_list.extend(new_list.iter());
        flush_storage_shards(writer, current_partial, &mut current_list, append_only, false)?;
    }

    flush_storage_shards(writer, current_partial, &mut current_list, append_only, true)?;

    Ok(())
}

#[cfg(all(unix, feature = "rocksdb"))]
fn flush_storage_shards<CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    partial_key: (alloy_primitives::Address, alloy_primitives::B256),
    list: &mut Vec<BlockNumber>,
    append_only: bool,
    flush_all: bool,
) -> Result<(), StageError>
where
    CURSOR: DbCursorRW<reth_db_api::tables::StoragesHistory>
        + DbCursorRO<reth_db_api::tables::StoragesHistory>,
    N: reth_primitives_traits::NodePrimitives,
{
    use reth_db_api::models::storage_sharded_key::StorageShardedKey;

    if list.len() > NUM_OF_INDICES_IN_SHARD || flush_all {
        let chunks =
            list.chunks(NUM_OF_INDICES_IN_SHARD).map(|c| c.to_vec()).collect::<Vec<Vec<u64>>>();

        let mut iter = chunks.into_iter().peekable();
        while let Some(chunk) = iter.next() {
            let mut highest = *chunk.last().expect("BlockNumberList shard chunk must be non-empty");

            if !flush_all && iter.peek().is_none() {
                *list = chunk;
            } else {
                if iter.peek().is_none() {
                    highest = u64::MAX;
                }
                let key = StorageShardedKey::new(partial_key.0, partial_key.1, highest);
                let value = BlockNumberList::new_pre_sorted(chunk);
                writer.put_storage_history(key, &value, append_only)?;
            }
        }
    }

    Ok(())
}

/// Load account history indices through an [`EitherWriter`] which routes writes to either
/// database or `RocksDB`.
///
/// Note: For `RocksDB`, when `append_only == false`, this function reads existing shards
/// from `RocksDB` and merges them with new indices.
#[cfg(all(unix, feature = "rocksdb"))]
pub(crate) fn load_account_history_indices_via_writer<CURSOR, N, Provider>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    mut collector: reth_etl::Collector<
        reth_db_api::models::ShardedKey<alloy_primitives::Address>,
        BlockNumberList,
    >,
    append_only: bool,
    rocksdb_provider: &Provider,
) -> Result<(), StageError>
where
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
    N: reth_primitives_traits::NodePrimitives,
    Provider: reth_provider::RocksDBProviderFactory,
{
    use reth_db_api::{models::ShardedKey, table::Decode};

    let mut current_partial = alloy_primitives::Address::default();
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    let rocksdb = rocksdb_provider.rocksdb_provider();

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = ShardedKey::<alloy_primitives::Address>::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_account_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices via EitherWriter");
        }

        let partial_key = sharded_key.key;

        if current_partial != partial_key {
            flush_account_shards(writer, current_partial, &mut current_list, append_only, true)?;
            current_partial = partial_key;
            current_list.clear();

            // If it's not the first sync, there might be an existing shard already in RocksDB,
            // so we need to merge it with the one coming from the collector
            if !append_only {
                let key = ShardedKey::new(partial_key, u64::MAX);
                if let Some(existing_list) =
                    rocksdb.get::<reth_db_api::tables::AccountsHistory>(key)?
                {
                    current_list.extend(existing_list.iter());
                }
            }
        }

        current_list.extend(new_list.iter());
        flush_account_shards(writer, current_partial, &mut current_list, append_only, false)?;
    }

    flush_account_shards(writer, current_partial, &mut current_list, append_only, true)?;

    Ok(())
}

#[cfg(all(unix, feature = "rocksdb"))]
fn flush_account_shards<CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    partial_key: alloy_primitives::Address,
    list: &mut Vec<BlockNumber>,
    append_only: bool,
    flush_all: bool,
) -> Result<(), StageError>
where
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
    N: reth_primitives_traits::NodePrimitives,
{
    use reth_db_api::models::ShardedKey;

    if list.len() > NUM_OF_INDICES_IN_SHARD || flush_all {
        let chunks =
            list.chunks(NUM_OF_INDICES_IN_SHARD).map(|c| c.to_vec()).collect::<Vec<Vec<u64>>>();

        let mut iter = chunks.into_iter().peekable();
        while let Some(chunk) = iter.next() {
            let mut highest = *chunk.last().expect("BlockNumberList shard chunk must be non-empty");

            if !flush_all && iter.peek().is_none() {
                *list = chunk;
            } else {
                if iter.peek().is_none() {
                    highest = u64::MAX;
                }
                let key = ShardedKey::new(partial_key, highest);
                let value = BlockNumberList::new_pre_sorted(chunk);
                writer.put_account_history(key, &value, append_only)?;
            }
        }
    }

    Ok(())
}

/// Generic helper to unwind history indices using `RocksDB`.
///
/// For each affected partial key, iterates shards in reverse order and:
/// 1. Deletes shards where all indices are `>= first_block_to_remove`
/// 2. For the boundary shard (contains indices both above and below the threshold), filters to keep
///    only indices `< first_block_to_remove` and rewrites as sentinel
/// 3. Preserves earlier shards unchanged
/// 4. If no indices remain after filtering, the key is fully removed
#[cfg(all(unix, feature = "rocksdb"))]
fn unwind_history_via_rocksdb_generic<Provider, T, PK, K, SK>(
    provider: &Provider,
    affected_keys: impl IntoIterator<Item = PK>,
    first_block_to_remove: BlockNumber,
    mk_start_key: impl Fn(&PK) -> K,
    mk_sentinel_key: impl Fn(&PK) -> K,
    shard_belongs_to_key: impl Fn(&K, &PK) -> bool,
) -> Result<usize, StageError>
where
    Provider: DBProvider + reth_provider::RocksDBProviderFactory,
    T: reth_db_api::table::Table<Key = K, Value = BlockNumberList>,
    K: Clone + AsRef<reth_db_api::models::ShardedKey<SK>>,
{
    let rocksdb = provider.rocksdb_provider();
    let tx = rocksdb.tx();
    let mut batch = rocksdb.batch();
    let mut count = 0;

    for pk in affected_keys {
        let start_key = mk_start_key(&pk);
        let mut shards = Vec::<(K, BlockNumberList)>::new();

        for entry in tx.iter_from::<T>(start_key)? {
            let (key, list) = entry?;
            if !shard_belongs_to_key(&key, &pk) {
                break;
            }
            shards.push((key, list));
        }

        if shards.is_empty() {
            continue;
        }

        count += 1;
        let mut reinsert: Option<Vec<u64>> = None;

        for (key, list) in shards.iter().rev() {
            let first = list.iter().next().expect("BlockNumberList shard must be non-empty");
            let highest = key.as_ref().highest_block_number;

            if first >= first_block_to_remove {
                // Entire shard is above threshold - delete it
                batch.delete::<T>(key.clone())?;
                continue;
            }

            // This shard has indices below the threshold
            if first_block_to_remove <= highest {
                // Boundary shard: filter and rewrite as sentinel
                batch.delete::<T>(key.clone())?;
                let filtered: Vec<u64> =
                    list.iter().take_while(|block| *block < first_block_to_remove).collect();
                if !filtered.is_empty() {
                    reinsert = Some(filtered);
                }
            }
            // Earlier shards are entirely below threshold - keep them unchanged
            break;
        }

        if let Some(indices) = reinsert {
            let sentinel_key = mk_sentinel_key(&pk);
            let new_list = BlockNumberList::new_pre_sorted(indices);
            batch.put::<T>(sentinel_key, &new_list)?;
        }
    }

    provider.set_pending_rocksdb_batch(batch.into_inner());
    Ok(count)
}

/// Unwind storage history indices using `RocksDB`.
///
/// Takes a set of affected (address, `storage_key`) pairs and removes all block numbers
/// >= `first_block_to_remove` from the history indices across ALL shards.
#[cfg(all(unix, feature = "rocksdb"))]
pub(crate) fn unwind_storage_history_via_rocksdb<Provider>(
    provider: &Provider,
    affected_keys: std::collections::BTreeSet<(alloy_primitives::Address, alloy_primitives::B256)>,
    first_block_to_remove: BlockNumber,
) -> Result<usize, StageError>
where
    Provider: DBProvider + reth_provider::RocksDBProviderFactory,
{
    use reth_db_api::models::storage_sharded_key::StorageShardedKey;

    unwind_history_via_rocksdb_generic::<_, reth_db_api::tables::StoragesHistory, _, _, _>(
        provider,
        affected_keys,
        first_block_to_remove,
        |(address, storage_key)| StorageShardedKey::new(*address, *storage_key, 0),
        |(address, storage_key)| StorageShardedKey::new(*address, *storage_key, u64::MAX),
        |storage_sharded_key, (address, storage_key)| {
            storage_sharded_key.address == *address &&
                storage_sharded_key.sharded_key.key == *storage_key
        },
    )
}

/// Unwind account history indices using `RocksDB`.
///
/// Takes a set of affected addresses and removes all block numbers
/// >= `first_block_to_remove` from the history indices across ALL shards.
#[cfg(all(unix, feature = "rocksdb"))]
pub(crate) fn unwind_account_history_via_rocksdb<Provider>(
    provider: &Provider,
    affected_addresses: std::collections::BTreeSet<alloy_primitives::Address>,
    first_block_to_remove: BlockNumber,
) -> Result<usize, StageError>
where
    Provider: DBProvider + reth_provider::RocksDBProviderFactory,
{
    use reth_db_api::models::ShardedKey;

    unwind_history_via_rocksdb_generic::<_, reth_db_api::tables::AccountsHistory, _, _, _>(
        provider,
        affected_addresses,
        first_block_to_remove,
        |address| ShardedKey::new(*address, 0),
        |address| ShardedKey::new(*address, u64::MAX),
        |sharded_key, address| sharded_key.key == *address,
    )
}

/// Called when database is ahead of static files. Attempts to find the first block we are missing
/// transactions for.
pub(crate) fn missing_static_data_error<Provider>(
    last_tx_num: TxNumber,
    static_file_provider: &StaticFileProvider<Provider::Primitives>,
    provider: &Provider,
    segment: StaticFileSegment,
) -> Result<StageError, ProviderError>
where
    Provider: BlockReader + StaticFileProviderFactory,
{
    let mut last_block =
        static_file_provider.get_highest_static_file_block(segment).unwrap_or_default();

    // To be extra safe, we make sure that the last tx num matches the last block from its indices.
    // If not, get it.
    loop {
        if let Some(indices) = provider.block_body_indices(last_block)? &&
            indices.last_tx_num() <= last_tx_num
        {
            break
        }
        if last_block == 0 {
            break
        }
        last_block -= 1;
    }

    let missing_block = Box::new(provider.sealed_header(last_block + 1)?.unwrap_or_default());

    Ok(StageError::MissingStaticFileData {
        block: Box::new(missing_block.block_with_parent()),
        segment,
    })
}
