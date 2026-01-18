//! Utils for `stages`.
use alloy_primitives::{Address, BlockNumber, TxNumber};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, AccountBeforeTx, ShardedKey},
    table::{Decompress, Table},
    transaction::{DbTx, DbTxMut},
    BlockNumberList, DatabaseError,
};
use reth_etl::Collector;
use reth_provider::{
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, ProviderError,
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

/// Backend operations for loading history indices.
///
/// This trait abstracts the storage backend (MDBX cursor, `EitherWriter`, etc.)
/// allowing the same loading algorithm to work with different backends.
pub(super) trait HistoryIndexWriter<K, P> {
    /// Get the last shard for a partial key (stored with `highest_block_number = u64::MAX`).
    fn get_last_shard(&mut self, partial: P) -> Result<Option<BlockNumberList>, StageError>;

    /// Write a shard to storage.
    fn put(&mut self, key: K, value: &BlockNumberList, append_only: bool)
        -> Result<(), StageError>;
}

/// Core implementation of history index loading.
///
/// This is the shared algorithm for loading history indices that works with any backend
/// implementing [`HistoryIndexWriter`].
///
/// # Invariant
///
/// The last shard for each partial key is always stored with `highest_block_number = u64::MAX`.
/// This allows `get_last_shard` to find it via `seek_exact(partial_key, u64::MAX)`.
pub(crate) fn load_history_indices_with<K, P, W>(
    mut collector: Collector<K, BlockNumberList>,
    append_only: bool,
    decode_key: impl Fn(Vec<u8>) -> Result<K, DatabaseError>,
    get_partial: impl Fn(&K) -> P,
    sharded_key_factory: impl Clone + Fn(P, u64) -> K,
    writer: &mut W,
) -> Result<(), StageError>
where
    K: reth_db_api::table::Key,
    P: Copy + Eq,
    W: HistoryIndexWriter<K, P>,
{
    let mut current_partial: Option<P> = None;
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = decode_key(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        let partial_key = get_partial(&sharded_key);

        if current_partial != Some(partial_key) {
            // Flush previous partial key's shards (skip if this is the first key)
            if let Some(prev) = current_partial {
                write_shards(prev, &mut current_list, &sharded_key_factory, true, |key, value| {
                    writer.put(key, &value, append_only)
                })?;
            }

            current_partial = Some(partial_key);
            current_list.clear();

            // Merge existing shard if not first sync
            if !append_only
                && let Some(last_database_shard) = writer.get_last_shard(partial_key)?
            {
                current_list.extend(last_database_shard.iter());
            }
        }

        current_list.extend(new_list.iter());
        write_shards(partial_key, &mut current_list, &sharded_key_factory, false, |key, value| {
            writer.put(key, &value, append_only)
        })?;
    }

    // Flush remaining shard
    if let Some(partial) = current_partial {
        write_shards(partial, &mut current_list, &sharded_key_factory, true, |key, value| {
            writer.put(key, &value, append_only)
        })?;
    }

    Ok(())
}

/// Wrapper for MDBX cursor that implements [`HistoryIndexWriter`].
struct CursorWriter<'a, H, C, F> {
    cursor: &'a mut C,
    sharded_key_factory: F,
    _table: std::marker::PhantomData<H>,
}

impl<H, P, C, F> HistoryIndexWriter<H::Key, P> for CursorWriter<'_, H, C, F>
where
    H: Table<Value = BlockNumberList>,
    P: Copy,
    C: DbCursorRO<H> + DbCursorRW<H>,
    F: Fn(P, u64) -> H::Key,
{
    fn get_last_shard(&mut self, partial: P) -> Result<Option<BlockNumberList>, StageError> {
        let key = (self.sharded_key_factory)(partial, u64::MAX);
        Ok(self.cursor.seek_exact(key)?.map(|(_, v)| v))
    }

    fn put(
        &mut self,
        key: H::Key,
        value: &BlockNumberList,
        append_only: bool,
    ) -> Result<(), StageError> {
        if append_only {
            self.cursor.append(key, value)?;
        } else {
            self.cursor.upsert(key, value)?;
        }
        Ok(())
    }
}

/// Given a [`Collector`] created by [`collect_history_indices`] it iterates all entries, loading
/// the indices into the database in shards.
///
/// Iterates over elements, grouping indices by their partial keys (e.g., `Address` or
/// `Address.StorageKey`). It flushes indices to disk when reaching a shard's max length
/// (`NUM_OF_INDICES_IN_SHARD`) or when the partial key changes.
pub(crate) fn load_history_indices<Provider, H, P>(
    provider: &Provider,
    collector: Collector<H::Key, H::Value>,
    append_only: bool,
    sharded_key_factory: impl Clone + Fn(P, u64) -> <H as Table>::Key,
    decode_key: impl Fn(Vec<u8>) -> Result<<H as Table>::Key, DatabaseError>,
    get_partial: impl Fn(&<H as Table>::Key) -> P,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut>,
    H: Table<Value = BlockNumberList>,
    P: Copy + Eq,
{
    let mut cursor = provider.tx_ref().cursor_write::<H>()?;
    let mut writer = CursorWriter::<H, _, _> {
        cursor: &mut cursor,
        sharded_key_factory: sharded_key_factory.clone(),
        _table: std::marker::PhantomData,
    };

    load_history_indices_with(
        collector,
        append_only,
        decode_key,
        get_partial,
        sharded_key_factory,
        &mut writer,
    )
}

/// Writes shards of block number indices using the provided put function.
///
/// Splits indices into chunks of `NUM_OF_INDICES_IN_SHARD` and writes them.
/// With `flush=true`, writes all shards (last uses `u64::MAX` as the highest block number).
/// With `flush=false`, keeps the last partial shard in `list` for continued accumulation.
pub(crate) fn write_shards<K, P>(
    partial_key: P,
    list: &mut Vec<u64>,
    sharded_key_factory: &impl Fn(P, u64) -> K,
    flush: bool,
    mut put: impl FnMut(K, BlockNumberList) -> Result<(), StageError>,
) -> Result<(), StageError>
where
    P: Copy,
{
    if list.len() <= NUM_OF_INDICES_IN_SHARD && !flush {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);
    let last_chunk_start = (num_chunks - 1) * NUM_OF_INDICES_IN_SHARD;

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;

        if !flush && is_last {
            // Keep last partial shard in memory using split_off to avoid element-by-element copy
            *list = list.split_off(last_chunk_start);
            return Ok(());
        }

        let highest = if is_last { u64::MAX } else { *chunk.last().expect("chunk is non-empty") };
        let key = sharded_key_factory(partial_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
        put(key, value)?;
    }

    if flush {
        list.clear();
    }

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
