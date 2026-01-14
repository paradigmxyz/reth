//! Utils for `stages`.
use alloy_primitives::{Address, BlockNumber, TxNumber, B256};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey,
        AccountBeforeTx, ShardedKey,
    },
    table::{Decompress, Table},
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList, DatabaseError,
};
use reth_etl::Collector;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    make_rocksdb_batch_arg, make_rocksdb_provider, providers::StaticFileProvider,
    register_rocksdb_batch, to_range, BlockReader, DBProvider, EitherWriter, ProviderError,
    RocksDBProviderFactory, StaticFileProviderFactory, StorageSettingsCache,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, NodePrimitivesProvider};
use reth_storage_errors::provider::ProviderResult;
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
    Ok::<(), StageError>(())
}

/// Generic shard-and-write helper used by both account and storage history loaders.
///
/// Chunks the list into shards, writes each shard via the provided write function,
/// and handles the last shard according to [`LoadMode`].
fn shard_and_write<F>(
    list: &mut Vec<BlockNumber>,
    mode: LoadMode,
    mut write_fn: F,
) -> Result<(), StageError>
where
    F: FnMut(Vec<u64>, BlockNumber) -> Result<(), StageError>,
{
    if list.len() <= NUM_OF_INDICES_IN_SHARD && !mode.is_flush() {
        return Ok(());
    }

    let chunks: Vec<_> = list.chunks(NUM_OF_INDICES_IN_SHARD).map(|c| c.to_vec()).collect();
    let mut iter = chunks.into_iter().peekable();

    while let Some(chunk) = iter.next() {
        let highest = *chunk.last().expect("at least one index");
        let is_last = iter.peek().is_none();

        if !mode.is_flush() && is_last {
            *list = chunk;
        } else {
            let highest = if is_last { u64::MAX } else { highest };
            write_fn(chunk, highest)?;
        }
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
        let last = indices.last().expect("qed");
        collector.insert(
            ShardedKey::new(address, *last),
            BlockNumberList::new_pre_sorted(indices.into_iter()),
        )?;
        Ok::<(), StageError>(())
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

/// Given a [`Collector`] created by [`collect_history_indices`] it iterates all entries, loading
/// the indices into the database in shards.
///
///  ## Process
/// Iterates over elements, grouping indices by their partial keys (e.g., `Address` or
/// `Address.StorageKey`). It flushes indices to disk when reaching a shard's max length
/// (`NUM_OF_INDICES_IN_SHARD`) or when the partial key changes, ensuring the last previous partial
/// key shard is stored.
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Loads storage history indices from a collector into the database using `EitherWriter`.
///
/// This is a specialized version of [`load_history_indices`] for `tables::StoragesHistory`
/// that supports writing to either `MDBX` or `RocksDB` based on storage settings.
#[allow(dead_code)]
pub(crate) fn load_storages_history_indices<Provider, P>(
    provider: &Provider,
    mut collector: Collector<
        <tables::StoragesHistory as Table>::Key,
        <tables::StoragesHistory as Table>::Value,
    >,
    append_only: bool,
    sharded_key_factory: impl Clone + Fn(P, u64) -> StorageShardedKey,
    decode_key: impl Fn(Vec<u8>) -> Result<StorageShardedKey, DatabaseError>,
    get_partial: impl Fn(StorageShardedKey) -> P,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut>
        + NodePrimitivesProvider
        + StorageSettingsCache
        + RocksDBProviderFactory,
    P: Copy + Default + Eq,
{
    // Create EitherWriter for storage history
    #[allow(clippy::let_unit_value)]
    let rocksdb = make_rocksdb_provider(provider);
    #[allow(clippy::let_unit_value)]
    let rocksdb_batch = make_rocksdb_batch_arg(&rocksdb);
    let mut writer = EitherWriter::new_storages_history(provider, rocksdb_batch)?;

    // Create read cursor for checking existing shards
    let mut read_cursor = provider.tx_ref().cursor_read::<tables::StoragesHistory>()?;

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
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing storage history indices");
        }

        let partial_key = get_partial(sharded_key);

        if current_partial != partial_key {
            // Flush last shard for previous partial key
            load_storages_history_shard(
                &mut writer,
                current_partial,
                &mut current_list,
                &sharded_key_factory,
                append_only,
                LoadMode::Flush,
            )?;

            current_partial = partial_key;
            current_list.clear();

            // If not first sync, merge with existing shard
            if !append_only &&
                let Some((_, last_database_shard)) =
                    read_cursor.seek_exact(sharded_key_factory(current_partial, u64::MAX))?
            {
                current_list.extend(last_database_shard.iter());
            }
        }

        current_list.extend(new_list.iter());
        load_storages_history_shard(
            &mut writer,
            current_partial,
            &mut current_list,
            &sharded_key_factory,
            append_only,
            LoadMode::KeepLast,
        )?;
    }

    // Flush remaining shard
    load_storages_history_shard(
        &mut writer,
        current_partial,
        &mut current_list,
        &sharded_key_factory,
        append_only,
        LoadMode::Flush,
    )?;

    // Register RocksDB batch for commit
    register_rocksdb_batch(provider, writer);

    Ok(())
}

/// Shard and insert storage history indices according to [`LoadMode`] and list length.
#[allow(dead_code)]
fn load_storages_history_shard<P, CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    partial_key: P,
    list: &mut Vec<BlockNumber>,
    sharded_key_factory: &impl Fn(P, BlockNumber) -> StorageShardedKey,
    _append_only: bool,
    mode: LoadMode,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
    P: Copy,
{
    shard_and_write(list, mode, |chunk, highest| {
        let key = sharded_key_factory(partial_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk);
        Ok(writer.put_storage_history(key, &value)?)
    })
}

/// Loads account history indices from a collector into the database using `EitherWriter`.
///
/// This is a specialized version of [`load_history_indices`] for `tables::AccountsHistory`
/// that supports writing to either `MDBX` or `RocksDB` based on storage settings.
#[allow(dead_code)]
pub(crate) fn load_accounts_history_indices<Provider, P>(
    provider: &Provider,
    mut collector: Collector<
        <tables::AccountsHistory as Table>::Key,
        <tables::AccountsHistory as Table>::Value,
    >,
    append_only: bool,
    sharded_key_factory: impl Clone + Fn(P, u64) -> ShardedKey<Address>,
    decode_key: impl Fn(Vec<u8>) -> Result<ShardedKey<Address>, DatabaseError>,
    get_partial: impl Fn(ShardedKey<Address>) -> P,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut>
        + NodePrimitivesProvider
        + StorageSettingsCache
        + RocksDBProviderFactory,
    P: Copy + Default + Eq,
{
    // Create EitherWriter for account history
    #[allow(clippy::let_unit_value)]
    let rocksdb = make_rocksdb_provider(provider);
    #[allow(clippy::let_unit_value)]
    let rocksdb_batch = make_rocksdb_batch_arg(&rocksdb);
    let mut writer = EitherWriter::new_accounts_history(provider, rocksdb_batch)?;

    // Create read cursor for checking existing shards
    let mut read_cursor = provider.tx_ref().cursor_read::<tables::AccountsHistory>()?;

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
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing account history indices");
        }

        let partial_key = get_partial(sharded_key);

        if current_partial != partial_key {
            // Flush last shard for previous partial key
            load_accounts_history_shard(
                &mut writer,
                current_partial,
                &mut current_list,
                &sharded_key_factory,
                append_only,
                LoadMode::Flush,
            )?;

            current_partial = partial_key;
            current_list.clear();

            // If not first sync, merge with existing shard
            if !append_only &&
                let Some((_, last_database_shard)) =
                    read_cursor.seek_exact(sharded_key_factory(current_partial, u64::MAX))?
            {
                current_list.extend(last_database_shard.iter());
            }
        }

        current_list.extend(new_list.iter());
        load_accounts_history_shard(
            &mut writer,
            current_partial,
            &mut current_list,
            &sharded_key_factory,
            append_only,
            LoadMode::KeepLast,
        )?;
    }

    // Flush remaining shard
    load_accounts_history_shard(
        &mut writer,
        current_partial,
        &mut current_list,
        &sharded_key_factory,
        append_only,
        LoadMode::Flush,
    )?;

    // Register RocksDB batch for commit
    register_rocksdb_batch(provider, writer);

    Ok(())
}

/// Shard and insert account history indices according to [`LoadMode`] and list length.
#[allow(dead_code)]
fn load_accounts_history_shard<P, CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    partial_key: P,
    list: &mut Vec<BlockNumber>,
    sharded_key_factory: &impl Fn(P, BlockNumber) -> ShardedKey<Address>,
    _append_only: bool,
    mode: LoadMode,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
    P: Copy,
{
    shard_and_write(list, mode, |chunk, highest| {
        let key = sharded_key_factory(partial_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk);
        Ok(writer.put_account_history(key, &value)?)
    })
}

/// Unwinds storage history shards using `EitherWriter` for `RocksDB` support.
///
/// This reimplements the shard unwinding logic with support for both MDBX and `RocksDB`.
/// Walks through shards for a given key, deleting those >= unwind point and preserving
/// indices below the unwind point.
#[allow(dead_code)]
pub(crate) fn unwind_storages_history_shards<CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    address: Address,
    storage_key: B256,
    block_number: BlockNumber,
) -> ProviderResult<()>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    writer.unwind_storage_history_shards(address, storage_key, block_number)
}

/// Unwinds account history shards using `EitherWriter` for `RocksDB` support.
///
/// This reimplements the shard unwinding logic with support for both MDBX and `RocksDB`.
/// Walks through shards for a given key, deleting those >= unwind point and preserving
/// indices below the unwind point.
#[allow(dead_code)]
pub(crate) fn unwind_accounts_history_shards<CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    address: Address,
    block_number: BlockNumber,
) -> ProviderResult<()>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    writer.unwind_account_history_shards(address, block_number)
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
