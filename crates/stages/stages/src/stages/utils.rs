//! Utils for `stages`.
use alloy_primitives::{map::AddressMap, Address, BlockNumber, TxNumber, B256};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey,
        AccountBeforeTx, AddressStorageKey, BlockNumberAddress, ShardedKey,
    },
    table::{Decode, Decompress, Table},
    tables,
    transaction::DbTx,
    BlockNumberList,
};
use reth_etl::Collector;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    prepare_history_shard_writes_parallel_vec, prepare_history_shard_writes_serial_vec,
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, EitherWriter,
    PreparedHistoryShardWrites, ProviderError, ProviderResult, RocksDBProviderFactory,
    ShardedHistoryTable, StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{collections::HashMap, hash::Hash, mem, ops::RangeBounds};
use tracing::info;

/// Number of blocks before pushing indices from cache to [`Collector`]
const DEFAULT_CACHE_THRESHOLD: u64 = 100_000;
/// Maximum number of logical keys held in a history collection cache.
const HISTORY_CACHE_KEY_LIMIT: usize = 500_000;
/// Maximum number of block numbers held in a history collection cache.
const HISTORY_CACHE_INDEX_LIMIT: usize = 8_000_000;
/// Maximum number of complete logical keys prepared in one batch.
const HISTORY_PREPARATION_KEY_LIMIT: usize = 65_536;
/// Maximum estimated decoded input size prepared in one batch.
const HISTORY_PREPARATION_BYTE_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct HistoryPreparationLimits {
    max_keys: usize,
    max_bytes: usize,
}

const HISTORY_PREPARATION_LIMITS: HistoryPreparationLimits = HistoryPreparationLimits {
    max_keys: HISTORY_PREPARATION_KEY_LIMIT,
    max_bytes: HISTORY_PREPARATION_BYTE_LIMIT,
};

const fn history_cache_limit_reached(keys: usize, indices: usize, blocks: u64) -> bool {
    keys >= HISTORY_CACHE_KEY_LIMIT ||
        indices >= HISTORY_CACHE_INDEX_LIMIT ||
        blocks >= DEFAULT_CACHE_THRESHOLD
}

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

    let mut collect = |cache: &mut HashMap<P, Vec<u64>>| {
        for (key, indices) in cache.drain() {
            let last = *indices.last().expect("qed");
            collector
                .insert(sharded_key_factory(key, last), BlockNumberList::new_pre_sorted(indices))?;
        }
        Ok::<(), StageError>(())
    };

    // observability
    let total_changesets = provider.tx_ref().entries::<CS>()?;
    let interval = (total_changesets / 1000).max(1);

    let mut cached_blocks = 0;
    let mut cached_indices = 0;
    let mut current_block_number = None;
    for (idx, entry) in changeset_cursor.walk_range(range)?.enumerate() {
        let (block_number, key) = partial_key_factory(entry?);

        if idx > 0 && idx.is_multiple_of(interval) && total_changesets > 1000 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.4}%", (idx as f64 / total_changesets as f64) * 100.0), "Collecting indices");
        }

        // Check limits before the first row of a new block so a flush never splits one block.
        if current_block_number != Some(block_number) {
            if current_block_number.is_some() &&
                history_cache_limit_reached(cache.len(), cached_indices, cached_blocks)
            {
                collect(&mut cache)?;
                cached_blocks = 0;
                cached_indices = 0;
            }
            current_block_number = Some(block_number);
            cached_blocks += 1;
        }

        cache.entry(key).or_default().push(block_number);
        cached_indices += 1;
    }
    collect(&mut cache)?;

    Ok(collector)
}

/// Allows collecting indices from a cache with a custom insert fn
fn collect_indices<K, F>(
    cache: impl Iterator<Item = (K, Vec<u64>)>,
    mut insert_fn: F,
) -> Result<(), StageError>
where
    F: FnMut(K, Vec<u64>) -> Result<(), StageError>,
{
    for (key, indices) in cache {
        insert_fn(key, indices)?
    }
    Ok(())
}

/// Collects account history indices using a provider that implements `ChangeSetReader`.
pub(crate) fn collect_account_history_indices<Provider>(
    provider: &Provider,
    range: impl RangeBounds<BlockNumber>,
    etl_config: &EtlConfig,
) -> Result<Collector<ShardedKey<Address>, BlockNumberList>, StageError>
where
    Provider: DBProvider + ChangeSetReader + StaticFileProviderFactory,
{
    let mut collector = Collector::new(etl_config.file_size, etl_config.dir.clone());
    let mut cache: AddressMap<Vec<u64>> = AddressMap::default();

    let mut insert_fn = |address: Address, indices: Vec<u64>| {
        let last = indices.last().expect("indices is non-empty");
        collector
            .insert(ShardedKey::new(address, *last), BlockNumberList::new_pre_sorted(indices))?;
        Ok(())
    };

    // Convert range bounds to concrete range
    let range = to_range(range);
    let start_block = range.start;

    // Use the new walker for lazy iteration over static file changesets
    let static_file_provider = provider.static_file_provider();

    let walker = static_file_provider.walk_account_changeset_range(range);

    let mut cached_blocks = 0;
    let mut cached_indices = 0;
    let mut current_block_number = None;

    for changeset_result in walker {
        let (block_number, AccountBeforeTx { address, .. }) = changeset_result?;

        // Check limits before the first row of a new block so a flush never splits one block.
        if current_block_number != Some(block_number) {
            if let Some(completed_block) = current_block_number &&
                history_cache_limit_reached(cache.len(), cached_indices, cached_blocks)
            {
                info!(
                    target: "sync::stages::index_history",
                    processed_blocks = completed_block.saturating_sub(start_block) + 1,
                    current_block = completed_block,
                    "Collecting indices"
                );
                collect_indices(cache.drain(), &mut insert_fn)?;
                cached_blocks = 0;
                cached_indices = 0;
            }
            current_block_number = Some(block_number);
            cached_blocks += 1;
        }

        cache.entry(address).or_default().push(block_number);
        cached_indices += 1;
    }
    collect_indices(cache.into_iter(), insert_fn)?;

    Ok(collector)
}

/// Collects storage history indices using a provider that implements `StorageChangeSetReader`.
pub(crate) fn collect_storage_history_indices<Provider>(
    provider: &Provider,
    range: impl RangeBounds<BlockNumber>,
    etl_config: &EtlConfig,
) -> Result<Collector<StorageShardedKey, BlockNumberList>, StageError>
where
    Provider: DBProvider + StorageChangeSetReader + StaticFileProviderFactory,
{
    let mut collector = Collector::new(etl_config.file_size, etl_config.dir.clone());
    let mut cache: HashMap<AddressStorageKey, Vec<u64>> = HashMap::default();

    let mut insert_fn = |key: AddressStorageKey, indices: Vec<u64>| {
        let last = indices.last().expect("qed");
        collector.insert(
            StorageShardedKey::new(key.0 .0, key.0 .1, *last),
            BlockNumberList::new_pre_sorted(indices),
        )?;
        Ok::<(), StageError>(())
    };

    let range = to_range(range);
    let start_block = range.start;
    let static_file_provider = provider.static_file_provider();

    let walker = static_file_provider.walk_storage_changeset_range(range);

    let mut cached_blocks = 0;
    let mut cached_indices = 0;
    let mut current_block_number = None;

    for changeset_result in walker {
        let (BlockNumberAddress((block_number, address)), storage) = changeset_result?;

        // Check limits before the first row of a new block so a flush never splits one block.
        if current_block_number != Some(block_number) {
            if let Some(completed_block) = current_block_number &&
                history_cache_limit_reached(cache.len(), cached_indices, cached_blocks)
            {
                info!(
                    target: "sync::stages::index_history",
                    processed_blocks = completed_block.saturating_sub(start_block) + 1,
                    current_block = completed_block,
                    "Collecting indices"
                );
                collect_indices(cache.drain(), &mut insert_fn)?;
                cached_blocks = 0;
                cached_indices = 0;
            }
            current_block_number = Some(block_number);
            cached_blocks += 1;
        }

        cache.entry(AddressStorageKey((address, storage.key))).or_default().push(block_number);
        cached_indices += 1;
    }

    collect_indices(cache.into_iter(), insert_fn)?;

    Ok(collector)
}

fn emit_grouped_history_key<T, F>(
    key: T::PartialKey,
    indices: Vec<BlockNumber>,
    grouped: &mut Vec<(T::PartialKey, Vec<BlockNumber>)>,
    grouped_bytes: &mut usize,
    limits: HistoryPreparationLimits,
    emit: &mut F,
) -> Result<(), StageError>
where
    T: ShardedHistoryTable,
    F: FnMut(Vec<(T::PartialKey, Vec<BlockNumber>)>) -> Result<(), StageError>,
{
    debug_assert!(
        indices.windows(2).all(|window| window[0] < window[1]),
        "indices must be strictly increasing: {indices:?}"
    );

    let group_bytes = history_group_bytes::<T>(&indices);
    let next_key_exceeds_limit = grouped.len() >= limits.max_keys;
    let next_key_exceeds_bytes = group_bytes > limits.max_bytes.saturating_sub(*grouped_bytes);

    if !grouped.is_empty() && (next_key_exceeds_limit || next_key_exceeds_bytes) {
        emit(mem::take(grouped))?;
        *grouped_bytes = 0;
    }

    *grouped_bytes = (*grouped_bytes).saturating_add(group_bytes);
    grouped.push((key, indices));
    Ok(())
}

const fn history_group_bytes<T: ShardedHistoryTable>(indices: &Vec<BlockNumber>) -> usize {
    mem::size_of::<(T::PartialKey, Vec<BlockNumber>)>()
        .saturating_add(indices.capacity().saturating_mul(mem::size_of::<BlockNumber>()))
}

/// Streams sorted ETL rows into bounded batches of complete logical keys.
fn for_each_grouped_history_chunk<T, F>(
    mut collector: Collector<T::Key, BlockNumberList>,
    limits: HistoryPreparationLimits,
    mut emit: F,
) -> Result<(), StageError>
where
    T: ShardedHistoryTable,
    F: FnMut(Vec<(T::PartialKey, Vec<BlockNumber>)>) -> Result<(), StageError>,
{
    let mut grouped = Vec::new();
    let mut grouped_bytes = 0;
    let mut current_key = None;
    let mut current_indices = Vec::new();
    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let key = T::Key::decode_owned(k)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Grouping indices");
        }

        let partial_key = T::partial_key(&key);
        if current_key != Some(partial_key) &&
            let Some(previous_key) = current_key.replace(partial_key)
        {
            emit_grouped_history_key::<T, _>(
                previous_key,
                mem::take(&mut current_indices),
                &mut grouped,
                &mut grouped_bytes,
                limits,
                &mut emit,
            )?;
        }

        if !grouped.is_empty() &&
            (grouped.len() >= limits.max_keys || grouped_bytes >= limits.max_bytes)
        {
            emit(mem::take(&mut grouped))?;
            grouped_bytes = 0;
        }

        let new_list = BlockNumberList::decompress_owned(v)?;
        let projected_len =
            current_indices.len().saturating_add(new_list.len().try_into().unwrap_or(usize::MAX));
        let projected_capacity = current_indices.capacity().max(projected_len);
        let projected_bytes = mem::size_of::<(T::PartialKey, Vec<BlockNumber>)>()
            .saturating_add(projected_capacity.saturating_mul(mem::size_of::<BlockNumber>()));
        if !grouped.is_empty() && projected_bytes > limits.max_bytes.saturating_sub(grouped_bytes) {
            emit(mem::take(&mut grouped))?;
            grouped_bytes = 0;
        }
        current_indices.extend(new_list.iter());

        if !grouped.is_empty() &&
            history_group_bytes::<T>(&current_indices) >
                limits.max_bytes.saturating_sub(grouped_bytes)
        {
            emit(mem::take(&mut grouped))?;
            grouped_bytes = 0;
        }
    }

    if let Some(key) = current_key {
        emit_grouped_history_key::<T, _>(
            key,
            current_indices,
            &mut grouped,
            &mut grouped_bytes,
            limits,
            &mut emit,
        )?;
    }
    if !grouped.is_empty() {
        emit(grouped)?;
    }

    Ok(())
}

fn prepare_grouped_history_writes<T, Provider>(
    grouped: Vec<(T::PartialKey, Vec<BlockNumber>)>,
    provider: &Provider,
    use_rocksdb: bool,
) -> Result<PreparedHistoryShardWrites<T>, StageError>
where
    T: ShardedHistoryTable,
    Provider: DBProvider + RocksDBProviderFactory,
{
    if use_rocksdb {
        let rocksdb = provider.rocksdb_provider();
        Ok(prepare_history_shard_writes_parallel_vec::<T, _>(grouped, |key| rocksdb.get::<T>(key))?)
    } else {
        Ok(prepare_history_shard_writes_serial_vec::<T, _>(grouped, |key| {
            provider.tx_ref().get::<T>(key).map_err(Into::into)
        })?)
    }
}

fn prepare_history_writes<T, Provider>(
    collector: Collector<T::Key, BlockNumberList>,
    provider: &Provider,
    use_rocksdb: bool,
    etl_config: &EtlConfig,
) -> Result<Collector<T::Key, BlockNumberList>, StageError>
where
    T: ShardedHistoryTable,
    Provider: DBProvider + RocksDBProviderFactory,
{
    let mut prepared = Collector::new(etl_config.file_size, etl_config.dir.clone());
    for_each_grouped_history_chunk::<T, _>(collector, HISTORY_PREPARATION_LIMITS, |grouped| {
        let writes = prepare_grouped_history_writes::<T, _>(grouped, provider, use_rocksdb)?;
        for (key, value) in writes.into_writes() {
            prepared.insert(key, value)?;
        }
        Ok(())
    })?;
    Ok(prepared)
}

/// Spools prepared account-history shards after merging each key's committed last shard.
///
/// Call this **before** opening a `RocksDB` write batch.
pub(crate) fn prepare_account_history_writes<Provider>(
    collector: Collector<ShardedKey<Address>, BlockNumberList>,
    provider: &Provider,
    use_rocksdb: bool,
    etl_config: &EtlConfig,
) -> Result<Collector<ShardedKey<Address>, BlockNumberList>, StageError>
where
    Provider: DBProvider + RocksDBProviderFactory,
{
    prepare_history_writes::<tables::AccountsHistory, _>(
        collector,
        provider,
        use_rocksdb,
        etl_config,
    )
}

/// Spools prepared storage-history shards after merging each key's committed last shard.
///
/// Call this **before** opening a `RocksDB` write batch.
pub(crate) fn prepare_storage_history_writes<Provider>(
    collector: Collector<StorageShardedKey, BlockNumberList>,
    provider: &Provider,
    use_rocksdb: bool,
    etl_config: &EtlConfig,
) -> Result<Collector<StorageShardedKey, BlockNumberList>, StageError>
where
    Provider: DBProvider + RocksDBProviderFactory,
{
    prepare_history_writes::<tables::StoragesHistory, _>(
        collector,
        provider,
        use_rocksdb,
        etl_config,
    )
}

/// Streams prepared history shards into serial puts, logging progress every 10%.
pub(crate) fn write_prepared_history_shards<T>(
    mut prepared: Collector<T::Key, BlockNumberList>,
    mut write: impl FnMut(T::Key, &BlockNumberList) -> ProviderResult<()>,
) -> ProviderResult<()>
where
    T: ShardedHistoryTable,
{
    let total_writes = prepared.len();
    let interval = (total_writes / 10).max(1);

    for (index, element) in prepared.iter().map_err(ProviderError::other)?.enumerate() {
        if index > 0 && index.is_multiple_of(interval) && total_writes > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_writes as f64) * 100.0), "Writing indices");
        }
        let (key, value) = element.map_err(ProviderError::other)?;
        let key = T::Key::decode_owned(key)?;
        let value = BlockNumberList::decompress_owned(value)?;
        write(key, &value)?;
    }

    Ok(())
}

/// Append-only empty-table loader for account history.
///
/// Streams the collector into `append_*` and never reads last shards.
pub(crate) fn load_account_history_append<N, CURSOR>(
    mut collector: Collector<ShardedKey<Address>, BlockNumberList>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    let mut current_address: Option<Address> = None;
    // Accumulator for block numbers where the current address changed.
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = ShardedKey::<Address>::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        let address = sharded_key.key;

        // When address changes, flush the previous address's shards and start fresh.
        if current_address != Some(address) {
            // Flush all remaining shards for the previous address (uses u64::MAX for last shard).
            if let Some(prev_addr) = current_address {
                flush_account_history_shards(prev_addr, &mut current_list, writer)?;
            }

            current_address = Some(address);
            current_list.clear();
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_account_history_shards_partial(address, &mut current_list, writer)?;
    }

    // Flush the final address's remaining shard.
    if let Some(addr) = current_address {
        flush_account_history_shards(addr, &mut current_list, writer)?;
    }

    Ok(())
}

/// Flushes complete shards for account history, keeping the trailing partial shard buffered.
///
/// Only flushes when we have more than one shard's worth of data, keeping the last
/// (possibly partial) shard for continued accumulation. This avoids writing a shard
/// that may need to be updated when more indices arrive.
fn flush_account_history_shards_partial<N, CURSOR>(
    address: Address,
    list: &mut Vec<u64>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    // Nothing to flush if we haven't filled a complete shard yet.
    if list.len() <= NUM_OF_INDICES_IN_SHARD {
        return Ok(());
    }

    let num_full_shards = list.len() / NUM_OF_INDICES_IN_SHARD;

    // Always keep at least one shard buffered for continued accumulation.
    // If len is exact multiple of shard size, keep the last full shard.
    let shards_to_flush = if list.len().is_multiple_of(NUM_OF_INDICES_IN_SHARD) {
        num_full_shards - 1
    } else {
        num_full_shards
    };

    if shards_to_flush == 0 {
        return Ok(());
    }

    // Split: flush the first N shards, keep the remainder buffered.
    let flush_len = shards_to_flush * NUM_OF_INDICES_IN_SHARD;
    let remainder = list.split_off(flush_len);

    // Write each complete shard with its highest block number as the key.
    for chunk in list.chunks(NUM_OF_INDICES_IN_SHARD) {
        let highest = *chunk.last().expect("chunk is non-empty");
        let key = ShardedKey::new(address, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
        writer.append_account_history(key, &value)?;
    }

    // Keep the remaining indices for the next iteration.
    *list = remainder;
    Ok(())
}

/// Flushes all remaining shards for account history, using `u64::MAX` for the last shard.
///
/// The `u64::MAX` key for the final shard is an invariant that allows `seek_exact(address,
/// u64::MAX)` to find the last shard during incremental sync for merging with new indices.
fn flush_account_history_shards<N, CURSOR>(
    address: Address,
    list: &mut Vec<u64>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    if list.is_empty() {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;

        // Use u64::MAX for the final shard's key. This invariant allows incremental sync
        // to find the last shard via seek_exact(address, u64::MAX) for merging.
        let highest = if is_last { u64::MAX } else { *chunk.last().expect("chunk is non-empty") };

        let key = ShardedKey::new(address, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
        writer.append_account_history(key, &value)?;
    }

    list.clear();
    Ok(())
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

/// Append-only empty-table loader for storage history.
///
/// Streams the collector into `append_*` and never reads last shards.
pub(crate) fn load_storage_history_append<N, CURSOR>(
    mut collector: Collector<StorageShardedKey, BlockNumberList>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    let mut current_key: Option<(Address, B256)> = None;
    // Accumulator for block numbers where the current (address, storage_key) changed.
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = StorageShardedKey::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        let partial_key = (sharded_key.address, sharded_key.sharded_key.key);

        // When (address, storage_key) changes, flush the previous key's shards and start fresh.
        if current_key != Some(partial_key) {
            // Flush all remaining shards for the previous key (uses u64::MAX for last shard).
            if let Some((prev_addr, prev_storage_key)) = current_key {
                flush_storage_history_shards(
                    prev_addr,
                    prev_storage_key,
                    &mut current_list,
                    writer,
                )?;
            }

            current_key = Some(partial_key);
            current_list.clear();
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_storage_history_shards_partial(
            partial_key.0,
            partial_key.1,
            &mut current_list,
            writer,
        )?;
    }

    // Flush the final key's remaining shard.
    if let Some((addr, storage_key)) = current_key {
        flush_storage_history_shards(addr, storage_key, &mut current_list, writer)?;
    }

    Ok(())
}

/// Flushes complete shards for storage history, keeping the trailing partial shard buffered.
///
/// Only flushes when we have more than one shard's worth of data, keeping the last
/// (possibly partial) shard for continued accumulation. This avoids writing a shard
/// that may need to be updated when more indices arrive.
fn flush_storage_history_shards_partial<N, CURSOR>(
    address: Address,
    storage_key: B256,
    list: &mut Vec<u64>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    // Nothing to flush if we haven't filled a complete shard yet.
    if list.len() <= NUM_OF_INDICES_IN_SHARD {
        return Ok(());
    }

    let num_full_shards = list.len() / NUM_OF_INDICES_IN_SHARD;

    // Always keep at least one shard buffered for continued accumulation.
    // If len is exact multiple of shard size, keep the last full shard.
    let shards_to_flush = if list.len().is_multiple_of(NUM_OF_INDICES_IN_SHARD) {
        num_full_shards - 1
    } else {
        num_full_shards
    };

    if shards_to_flush == 0 {
        return Ok(());
    }

    // Split: flush the first N shards, keep the remainder buffered.
    let flush_len = shards_to_flush * NUM_OF_INDICES_IN_SHARD;
    let remainder = list.split_off(flush_len);

    // Write each complete shard with its highest block number as the key.
    for chunk in list.chunks(NUM_OF_INDICES_IN_SHARD) {
        let highest = *chunk.last().expect("chunk is non-empty");
        let key = StorageShardedKey::new(address, storage_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
        writer.append_storage_history(key, &value)?;
    }

    // Keep the remaining indices for the next iteration.
    *list = remainder;
    Ok(())
}

/// Flushes all remaining shards for storage history, using `u64::MAX` for the last shard.
///
/// The `u64::MAX` key for the final shard is an invariant that allows
/// `seek_exact(address, storage_key, u64::MAX)` to find the last shard during incremental
/// sync for merging with new indices.
fn flush_storage_history_shards<N, CURSOR>(
    address: Address,
    storage_key: B256,
    list: &mut Vec<u64>,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    if list.is_empty() {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;

        // Use u64::MAX for the final shard's key. This invariant allows incremental sync
        // to find the last shard via seek_exact(address, storage_key, u64::MAX) for merging.
        let highest = if is_last { u64::MAX } else { *chunk.last().expect("chunk is non-empty") };

        let key = StorageShardedKey::new(address, storage_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
        writer.append_storage_history(key, &value)?;
    }

    list.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    fn list(indices: &[u64]) -> BlockNumberList {
        BlockNumberList::new(indices.iter().copied()).unwrap()
    }

    fn grouped_account_chunks(
        collector: Collector<ShardedKey<Address>, BlockNumberList>,
        limits: HistoryPreparationLimits,
    ) -> Vec<Vec<(Address, Vec<BlockNumber>)>> {
        let mut chunks = Vec::new();
        for_each_grouped_history_chunk::<tables::AccountsHistory, _>(collector, limits, |chunk| {
            chunks.push(chunk);
            Ok(())
        })
        .unwrap();
        chunks
    }

    #[test]
    fn grouping_coalesces_one_logical_key_across_etl_files() {
        let address = address!("0x0000000000000000000000000000000000000001");
        // Every entry exceeds this tiny buffer, forcing it into a separate ETL file.
        let mut collector = Collector::new(1, None);
        collector.insert(ShardedKey::new(address, 2), list(&[1, 2])).unwrap();
        collector.insert(ShardedKey::new(address, 4), list(&[3, 4])).unwrap();

        let chunks = grouped_account_chunks(
            collector,
            HistoryPreparationLimits { max_keys: 10, max_bytes: usize::MAX },
        );
        assert_eq!(chunks, vec![vec![(address, vec![1, 2, 3, 4])]]);
    }

    #[test]
    fn storage_grouping_coalesces_one_logical_key_across_etl_files() {
        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000002");
        let mut collector = Collector::new(1, None);
        collector.insert(StorageShardedKey::new(address, slot, 2), list(&[1, 2])).unwrap();
        collector.insert(StorageShardedKey::new(address, slot, 4), list(&[3, 4])).unwrap();

        let mut chunks = Vec::new();
        for_each_grouped_history_chunk::<tables::StoragesHistory, _>(
            collector,
            HistoryPreparationLimits { max_keys: 10, max_bytes: usize::MAX },
            |chunk| {
                chunks.push(chunk);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(chunks, vec![vec![((address, slot), vec![1, 2, 3, 4])]]);
    }

    #[test]
    fn grouping_never_splits_a_key_at_key_limit() {
        let mut collector = Collector::new(1, None);
        for byte in 1u8..=5 {
            let address = Address::repeat_byte(byte);
            collector.insert(ShardedKey::new(address, u64::MAX), list(&[u64::from(byte)])).unwrap();
        }

        let chunks = grouped_account_chunks(
            collector,
            HistoryPreparationLimits { max_keys: 2, max_bytes: usize::MAX },
        );
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2, 1]);
        assert!(chunks.into_iter().flatten().all(|(_, indices)| indices.len() == 1));
    }

    #[test]
    fn grouping_processes_an_over_budget_key_alone() {
        let mut collector = Collector::new(1, None);
        for byte in 1u8..=2 {
            let address = Address::repeat_byte(byte);
            collector.insert(ShardedKey::new(address, u64::MAX), list(&[u64::from(byte)])).unwrap();
        }

        let chunks = grouped_account_chunks(
            collector,
            HistoryPreparationLimits { max_keys: usize::MAX, max_bytes: 1 },
        );
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 1]);
    }
}
