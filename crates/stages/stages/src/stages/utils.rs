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
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, EitherWriter, ProviderError,
    ProviderResult, StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::ChangeSetReader;
use std::{collections::HashMap, hash::Hash, ops::RangeBounds};
use tracing::info;

// ================================================================================================
// HistoryShardWriter trait - unifies account and storage history writing
// ================================================================================================

/// A trait for writing history shards to either MDBX or RocksDB.
///
/// This trait abstracts the differences between account history and storage history,
/// allowing a single generic `load_history` function to handle both.
pub(crate) trait HistoryShardWriter {
    /// The partial key type (Address for accounts, (Address, B256) for storage).
    type PartialKey: Copy + Eq;

    /// The full sharded key type.
    type ShardedKey;

    /// Decodes a sharded key from raw bytes.
    fn decode_key(bytes: Vec<u8>) -> Result<Self::ShardedKey, DatabaseError>;

    /// Extracts the partial key from a sharded key.
    fn partial_key(sharded_key: &Self::ShardedKey) -> Self::PartialKey;

    /// Creates a sharded key from a partial key and block number.
    fn make_sharded_key(partial: Self::PartialKey, block_number: u64) -> Self::ShardedKey;

    /// Appends a history entry (for first sync - more efficient).
    fn append(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()>;

    /// Upserts a history entry (for incremental sync).
    fn upsert(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()>;

    /// Gets the last shard for a partial key (keyed with `u64::MAX`).
    fn get_last_shard(
        &mut self,
        partial: Self::PartialKey,
    ) -> ProviderResult<Option<BlockNumberList>>;
}

// ================================================================================================
// Account History implementation
// ================================================================================================

impl<'a, CURSOR, N: NodePrimitives> HistoryShardWriter for EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    type PartialKey = Address;
    type ShardedKey = ShardedKey<Address>;

    fn decode_key(bytes: Vec<u8>) -> Result<Self::ShardedKey, DatabaseError> {
        use reth_db_api::table::Decode;
        ShardedKey::<Address>::decode_owned(bytes)
    }

    fn partial_key(sharded_key: &Self::ShardedKey) -> Self::PartialKey {
        sharded_key.key
    }

    fn make_sharded_key(partial: Self::PartialKey, block_number: u64) -> Self::ShardedKey {
        ShardedKey::new(partial, block_number)
    }

    fn append(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()> {
        self.append_account_history(key, value)
    }

    fn upsert(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()> {
        self.upsert_account_history(key, value)
    }

    fn get_last_shard(
        &mut self,
        partial: Self::PartialKey,
    ) -> ProviderResult<Option<BlockNumberList>> {
        self.get_last_account_history_shard(partial)
    }
}

// ================================================================================================
// Storage History implementation
// ================================================================================================

/// Wrapper type to implement HistoryShardWriter for storage history.
///
/// We need this wrapper because EitherWriter's CURSOR type determines which table it writes to,
/// and we can't have two conflicting trait implementations on the same type.
pub(crate) struct StorageHistoryWriter<'a, 'b, CURSOR, N: NodePrimitives> {
    inner: &'b mut EitherWriter<'a, CURSOR, N>,
}

impl<'a, 'b, CURSOR, N: NodePrimitives> StorageHistoryWriter<'a, 'b, CURSOR, N> {
    pub fn new(writer: &'b mut EitherWriter<'a, CURSOR, N>) -> Self {
        Self { inner: writer }
    }
}

impl<'a, 'b, CURSOR, N: NodePrimitives> HistoryShardWriter
    for StorageHistoryWriter<'a, 'b, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    type PartialKey = (Address, B256);
    type ShardedKey = StorageShardedKey;

    fn decode_key(bytes: Vec<u8>) -> Result<Self::ShardedKey, DatabaseError> {
        use reth_db_api::table::Decode;
        StorageShardedKey::decode_owned(bytes)
    }

    fn partial_key(sharded_key: &Self::ShardedKey) -> Self::PartialKey {
        (sharded_key.address, sharded_key.sharded_key.key)
    }

    fn make_sharded_key(partial: Self::PartialKey, block_number: u64) -> Self::ShardedKey {
        StorageShardedKey::new(partial.0, partial.1, block_number)
    }

    fn append(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()> {
        self.inner.append_storage_history(key, value)
    }

    fn upsert(&mut self, key: Self::ShardedKey, value: &BlockNumberList) -> ProviderResult<()> {
        self.inner.upsert_storage_history(key, value)
    }

    fn get_last_shard(
        &mut self,
        partial: Self::PartialKey,
    ) -> ProviderResult<Option<BlockNumberList>> {
        self.inner.get_last_storage_history_shard(partial.0, partial.1)
    }
}

// ================================================================================================
// Generic history loading function
// ================================================================================================

/// Loads history indices into the database via a [`HistoryShardWriter`].
///
/// This is the unified implementation for both account and storage history loading,
/// replacing the duplicated `load_account_history` and `load_storage_history` functions.
///
/// ## Process
/// Iterates over elements from the collector, grouping indices by their partial keys.
/// It flushes indices to disk when reaching a shard's max length (`NUM_OF_INDICES_IN_SHARD`)
/// or when the partial key changes, ensuring the last previous shard is stored.
///
/// Uses `Option<PartialKey>` instead of default values as the sentinel to avoid
/// incorrectly treating zero values as "no previous key".
pub(crate) fn load_history<W, K, V>(
    mut collector: Collector<K, V>,
    append_only: bool,
    writer: &mut W,
) -> Result<(), StageError>
where
    W: HistoryShardWriter,
    V: AsRef<[u8]>,
{
    let mut current_key: Option<W::PartialKey> = None;
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = W::decode_key(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(target: "sync::stages::index_history", progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
        }

        let partial_key = W::partial_key(&sharded_key);

        // When partial key changes, flush the previous key's shards and start fresh.
        if current_key != Some(partial_key) {
            // Flush all remaining shards for the previous key (uses u64::MAX for last shard).
            if let Some(prev_key) = current_key {
                flush_history_shards(prev_key, &mut current_list, append_only, writer)?;
            }

            current_key = Some(partial_key);
            current_list.clear();

            // On incremental sync, merge with the existing last shard from the database.
            // The last shard is stored with u64::MAX so we can find it.
            if !append_only {
                if let Some(last_shard) = writer.get_last_shard(partial_key)? {
                    current_list.extend(last_shard.iter());
                }
            }
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_history_shards_partial(partial_key, &mut current_list, append_only, writer)?;
    }

    // Flush the final key's remaining shard.
    if let Some(key) = current_key {
        flush_history_shards(key, &mut current_list, append_only, writer)?;
    }

    Ok(())
}

/// Flushes complete shards, keeping the trailing partial shard buffered.
///
/// Only flushes when we have more than one shard's worth of data, keeping the last
/// (possibly partial) shard for continued accumulation.
fn flush_history_shards_partial<W: HistoryShardWriter>(
    partial_key: W::PartialKey,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut W,
) -> Result<(), StageError> {
    if list.len() <= NUM_OF_INDICES_IN_SHARD {
        return Ok(());
    }

    let num_full_shards = list.len() / NUM_OF_INDICES_IN_SHARD;

    // Always keep at least one shard buffered for continued accumulation.
    let shards_to_flush = if list.len().is_multiple_of(NUM_OF_INDICES_IN_SHARD) {
        num_full_shards - 1
    } else {
        num_full_shards
    };

    if shards_to_flush == 0 {
        return Ok(());
    }

    let flush_len = shards_to_flush * NUM_OF_INDICES_IN_SHARD;
    let remainder = list.split_off(flush_len);

    for chunk in list.chunks(NUM_OF_INDICES_IN_SHARD) {
        let highest = *chunk.last().expect("chunk is non-empty");
        let key = W::make_sharded_key(partial_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());

        if append_only {
            writer.append(key, &value)?;
        } else {
            writer.upsert(key, &value)?;
        }
    }

    *list = remainder;
    Ok(())
}

/// Flushes all remaining shards, using `u64::MAX` for the last shard.
///
/// The `u64::MAX` key for the final shard allows incremental sync to find
/// the last shard via seek_exact for merging with new indices.
fn flush_history_shards<W: HistoryShardWriter>(
    partial_key: W::PartialKey,
    list: &mut Vec<u64>,
    append_only: bool,
    writer: &mut W,
) -> Result<(), StageError> {
    if list.is_empty() {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;
        let highest = if is_last { u64::MAX } else { *chunk.last().expect("chunk is non-empty") };

        let key = W::make_sharded_key(partial_key, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());

        if append_only {
            writer.append(key, &value)?;
        } else {
            writer.upsert(key, &value)?;
        }
    }

    list.clear();
    Ok(())
}

/// Convenience function for loading account history.
pub(crate) fn load_account_history<N, CURSOR>(
    collector: Collector<ShardedKey<Address>, BlockNumberList>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    load_history(collector, append_only, writer)
}

/// Convenience function for loading storage history.
pub(crate) fn load_storage_history<N, CURSOR>(
    collector: Collector<StorageShardedKey, BlockNumberList>,
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    let mut storage_writer = StorageHistoryWriter::new(writer);
    load_history(collector, append_only, &mut storage_writer)
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
