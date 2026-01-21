//! Utils for `stages`.
use alloy_primitives::{Address, BlockNumber, TxNumber};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey,
        AccountBeforeTx, ShardedKey,
    },
    table::{Decompress, Key, Table},
    transaction::DbTx,
    BlockNumberList,
};
use reth_etl::Collector;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, EitherWriter, ProviderError,
    ProviderResult, StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::ChangeSetReader;
use std::{collections::HashMap, hash::Hash, ops::RangeBounds};
use tracing::info;

/// Generic function that loads sharded history indices from a collector into the database.
///
/// This function implements the core shard-loading algorithm used by both account and storage
/// history stages. It handles:
/// - Grouping indices by their prefix key (address or address+storage_key)
/// - Flushing complete shards while buffering the trailing partial shard
/// - Using `u64::MAX` for the final shard's key (incremental sync invariant)
/// - Append vs upsert semantics based on `append_only` flag
///
/// The closure parameters provide the table-specific operations, enabling code reuse
/// without trait overhead.
fn load_sharded_history<W, PrefixKey, TableKey, MakeKey, KeyPrefix, GetLastShard, WriteShard>(
    collector: &mut Collector<TableKey, BlockNumberList>,
    append_only: bool,
    writer: &mut W,
    make_key: MakeKey,
    key_prefix: KeyPrefix,
    get_last_shard: GetLastShard,
    write_shard: WriteShard,
) -> Result<(), StageError>
where
    PrefixKey: Copy + Eq,
    TableKey: Key,
    MakeKey: Fn(PrefixKey, u64) -> TableKey,
    KeyPrefix: Fn(&TableKey) -> PrefixKey,
    GetLastShard: Fn(&mut W, PrefixKey) -> ProviderResult<Option<BlockNumberList>>,
    WriteShard: Fn(&mut W, TableKey, &BlockNumberList, bool) -> ProviderResult<()>,
{
    let mut current_prefix: Option<PrefixKey> = None;
    // Accumulator for block numbers where the current prefix changed.
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = TableKey::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        let prefix = key_prefix(&sharded_key);

        // When prefix changes, flush the previous prefix's shards and start fresh.
        if current_prefix != Some(prefix) {
            // Flush all remaining shards for the previous prefix (uses u64::MAX for last shard).
            if let Some(prev_prefix) = current_prefix {
                flush_shards(
                    prev_prefix,
                    &mut current_list,
                    append_only,
                    writer,
                    &make_key,
                    &write_shard,
                )?;
            }

            current_prefix = Some(prefix);
            current_list.clear();

            // On incremental sync, merge with the existing last shard from the database.
            // The last shard is stored with key (prefix, u64::MAX) so we can find it.
            if !append_only && let Some(last_shard) = get_last_shard(writer, prefix)? {
                current_list.extend(last_shard.iter());
            }
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_shards_partial(
            prefix,
            &mut current_list,
            append_only,
            writer,
            &make_key,
            &write_shard,
        )?;
    }

    // Flush the final prefix's remaining shard.
    if let Some(prefix) = current_prefix {
        flush_shards(prefix, &mut current_list, append_only, writer, &make_key, &write_shard)?;
    }

    Ok(())
}

/// Flushes complete shards, keeping the trailing partial shard buffered.
///
/// Only flushes when we have more than one shard's worth of data, keeping the last
/// (possibly partial) shard for continued accumulation. This avoids writing a shard
/// that may need to be updated when more indices arrive.
fn flush_shards_partial<W, PrefixKey, TableKey, MakeKey, WriteShard>(
    prefix: PrefixKey,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut W,
    make_key: &MakeKey,
    write_shard: &WriteShard,
) -> Result<(), StageError>
where
    PrefixKey: Copy,
    MakeKey: Fn(PrefixKey, u64) -> TableKey,
    WriteShard: Fn(&mut W, TableKey, &BlockNumberList, bool) -> ProviderResult<()>,
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
        let key = make_key(prefix, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());

        write_shard(writer, key, &value, append_only)?;
    }

    // Keep the remaining indices for the next iteration.
    *list = remainder;
    Ok(())
}

/// Flushes all remaining shards, using `u64::MAX` for the last shard.
///
/// The `u64::MAX` key for the final shard is an invariant that allows
/// `seek_exact(prefix, u64::MAX)` to find the last shard during incremental
/// sync for merging with new indices.
fn flush_shards<W, PrefixKey, TableKey, MakeKey, WriteShard>(
    prefix: PrefixKey,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut W,
    make_key: &MakeKey,
    write_shard: &WriteShard,
) -> Result<(), StageError>
where
    PrefixKey: Copy,
    MakeKey: Fn(PrefixKey, u64) -> TableKey,
    WriteShard: Fn(&mut W, TableKey, &BlockNumberList, bool) -> ProviderResult<()>,
{
    if list.is_empty() {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;

        // Use u64::MAX for the final shard's key. This invariant allows incremental sync
        // to find the last shard via seek_exact(prefix, u64::MAX) for merging.
        let highest = if is_last { u64::MAX } else { *chunk.last().expect("chunk is non-empty") };

        let key = make_key(prefix, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());

        write_shard(writer, key, &value, append_only)?;
    }

    list.clear();
    Ok(())
}

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

    let mut collect = |cache: &mut HashMap<P, Vec<u64>>| {
        for (key, indices) in cache.drain() {
            let last = *indices.last().expect("qed");
            collector.insert(
                sharded_key_factory(key, last),
                BlockNumberList::new_pre_sorted(indices.into_iter()),
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
                collect(&mut cache)?;
                flush_counter = 0;
            }
        }
    }
    collect(&mut cache)?;

    Ok(collector)
}

/// Allows collecting indices from a cache with a custom insert fn
fn collect_indices<F>(
    cache: impl Iterator<Item = (Address, Vec<u64>)>,
    mut insert_fn: F,
) -> Result<(), StageError>
where
    F: FnMut(Address, Vec<u64>) -> Result<(), StageError>,
{
    for (address, indices) in cache {
        insert_fn(address, indices)?
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
    let mut cache: HashMap<Address, Vec<u64>> = HashMap::default();

    let mut insert_fn = |address: Address, indices: Vec<u64>| {
        let last = indices.last().expect("indices is non-empty");
        collector.insert(
            ShardedKey::new(address, *last),
            BlockNumberList::new_pre_sorted(indices.into_iter()),
        )?;
        Ok(())
    };

    // Convert range bounds to concrete range
    let range = to_range(range);

    // Use the new walker for lazy iteration over static file changesets
    let static_file_provider = provider.static_file_provider();

    // Get total count for progress reporting
    let total_changesets = static_file_provider.account_changeset_count()?;
    let interval = (total_changesets / 1000).max(1);

    let walker = static_file_provider.walk_account_changeset_range(range);

    let mut flush_counter = 0;
    let mut current_block_number = u64::MAX;

    for (idx, changeset_result) in walker.enumerate() {
        let (block_number, AccountBeforeTx { address, .. }) = changeset_result?;
        cache.entry(address).or_default().push(block_number);

        if idx > 0 && idx % interval == 0 && total_changesets > 1000 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.4}%", (idx as f64 / total_changesets as f64) * 100.0), "Collecting indices");
        }

        if block_number != current_block_number {
            current_block_number = block_number;
            flush_counter += 1;
        }

        if flush_counter > DEFAULT_CACHE_THRESHOLD {
            collect_indices(cache.drain(), &mut insert_fn)?;
            flush_counter = 0;
        }
    }
    collect_indices(cache.into_iter(), insert_fn)?;

    Ok(collector)
}

/// Loads account history indices into the database via `EitherWriter`.
///
/// Works with [`EitherWriter`] to support both MDBX and `RocksDB` backends.
///
/// ## Process
/// Iterates over elements, grouping indices by their address. It flushes indices to disk
/// when reaching a shard's max length (`NUM_OF_INDICES_IN_SHARD`) or when the address changes,
/// ensuring the last previous address shard is stored.
///
/// Uses `Option<Address>` instead of `Address::default()` as the sentinel to avoid
/// incorrectly treating `Address::ZERO` as "no previous address".
pub(crate) fn load_account_history<N, CURSOR>(
    mut collector: Collector<ShardedKey<Address>, BlockNumberList>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
{
    load_sharded_history(
        &mut collector,
        append_only,
        writer,
        |address, highest_block| ShardedKey::new(address, highest_block),
        |key: &ShardedKey<Address>| key.key,
        |writer, address| writer.get_last_account_history_shard(address),
        |writer, key, value, append| {
            if append {
                writer.append_account_history(key, value)
            } else {
                writer.upsert_account_history(key, value)
            }
        },
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

/// Loads storage history indices into the database via `EitherWriter`.
///
/// Works with [`EitherWriter`] to support both MDBX and `RocksDB` backends.
///
/// ## Process
/// Iterates over elements, grouping indices by their (address, `storage_key`) pairs. It flushes
/// indices to disk when reaching a shard's max length (`NUM_OF_INDICES_IN_SHARD`) or when the
/// (address, `storage_key`) pair changes, ensuring the last previous shard is stored.
///
/// Uses `Option<(Address, B256)>` instead of default values as the sentinel to avoid
/// incorrectly treating `(Address::ZERO, B256::ZERO)` as "no previous key".
pub(crate) fn load_storage_history<N, CURSOR>(
    mut collector: Collector<StorageShardedKey, BlockNumberList>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::StoragesHistory>
        + DbCursorRO<reth_db_api::tables::StoragesHistory>,
{
    load_sharded_history(
        &mut collector,
        append_only,
        writer,
        |(address, storage_key), highest_block| {
            StorageShardedKey::new(address, storage_key, highest_block)
        },
        |key: &StorageShardedKey| (key.address, key.sharded_key.key),
        |writer, (address, storage_key)| writer.get_last_storage_history_shard(address, storage_key),
        |writer, key, value, append| {
            if append {
                writer.append_storage_history(key, value)
            } else {
                writer.upsert_storage_history(key, value)
            }
        },
    )
}
