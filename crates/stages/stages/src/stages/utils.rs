//! Utils for `stages`.
use alloy_primitives::{Address, BlockNumber, TxNumber};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, AccountBeforeTx, ShardedKey},
    table::{Decode, Decompress, Table},
    transaction::{DbTx, DbTxMut},
    BlockNumberList, DatabaseError,
};
use reth_etl::Collector;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, EitherWriter, ProviderError,
    StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::ChangeSetReader;
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

/// Loads account history indices into the database via `EitherWriter`.
///
/// Similar to [`load_history_indices`] but works with [`EitherWriter`] to support
/// both MDBX and `RocksDB` backends.
///
/// ## Process
/// Iterates over elements, grouping indices by their address. It flushes indices to disk
/// when reaching a shard's max length (`NUM_OF_INDICES_IN_SHARD`) or when the address changes,
/// ensuring the last previous address shard is stored.
///
/// Uses `Option<Address>` instead of `Address::default()` as the sentinel to avoid
/// incorrectly treating `Address::ZERO` as "no previous address".
pub(crate) fn load_account_history_via_writer<N, CURSOR>(
    mut collector: Collector<ShardedKey<Address>, BlockNumberList>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
{
    // Track current address being processed. Using Option<Address> instead of Address::default()
    // to correctly handle Address::ZERO (which would otherwise be treated as "no address").
    let mut current_address: Option<Address> = None;
    // Accumulator for block numbers where the current address changed.
    let mut current_list = Vec::<u64>::new();

    // Progress reporting setup.
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
                flush_account_history_shards(prev_addr, &mut current_list, append_only, writer)?;
            }

            current_address = Some(address);
            current_list.clear();

            // On incremental sync, merge with the existing last shard from the database.
            // The last shard is stored with key (address, u64::MAX) so we can find it.
            if !append_only &&
                let Some(last_shard) = writer.get_last_account_history_shard(address)?
            {
                current_list.extend(last_shard.iter());
            }
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_account_history_shards_partial(address, &mut current_list, append_only, writer)?;
    }

    // Flush the final address's remaining shard.
    if let Some(addr) = current_address {
        flush_account_history_shards(addr, &mut current_list, append_only, writer)?;
    }

    Ok(())
}

/// Flushes complete shards for account history, keeping the trailing partial shard buffered.
///
/// Only flushes when we have more than one shard's worth of data, keeping the last
/// (possibly partial) shard for continued accumulation. This avoids writing a shard
/// that may need to be updated when more indices arrive.
///
/// Equivalent to [`load_indices`] with [`LoadMode::KeepLast`].
fn flush_account_history_shards_partial<N, CURSOR>(
    address: Address,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
{
    // Nothing to flush if we haven't filled a complete shard yet.
    if list.len() <= NUM_OF_INDICES_IN_SHARD {
        return Ok(());
    }

    // Calculate how many complete shards we have.
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

        if append_only {
            writer.append_account_history(key, &value)?;
        } else {
            writer.upsert_account_history(key, &value)?;
        }
    }

    // Keep the remaining indices for the next iteration.
    *list = remainder;
    Ok(())
}

/// Flushes all remaining shards for account history, using `u64::MAX` for the last shard.
///
/// The `u64::MAX` key for the final shard is an invariant that allows `seek_exact(address,
/// u64::MAX)` to find the last shard during incremental sync for merging with new indices.
///
/// Equivalent to [`load_indices`] with [`LoadMode::Flush`].
fn flush_account_history_shards<N, CURSOR>(
    address: Address,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::AccountsHistory>
        + DbCursorRO<reth_db_api::tables::AccountsHistory>,
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

        if append_only {
            writer.append_account_history(key, &value)?;
        } else {
            writer.upsert_account_history(key, &value)?;
        }
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
