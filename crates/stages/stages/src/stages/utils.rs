//! Utils for `stages`.
use alloy_primitives::{
    map::{AddressMap, HashMap},
    Address, BlockNumber, TxNumber, B256,
};
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey,
        AccountBeforeTx, AddressStorageKey, BlockNumberAddress, ShardedKey,
    },
    table::{Decode, Decompress, Key, Table},
    transaction::DbTx,
    BlockNumberList,
};
use reth_etl::Collector;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, to_range, BlockReader, DBProvider, EitherWriter, ProviderError,
    StaticFileProviderFactory,
};
use reth_stages_api::StageError;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{
    hash::{BuildHasher, Hash},
    mem::size_of,
    ops::RangeBounds,
};
use tracing::{debug, info};

/// Number of blocks before pushing indices from cache to [`Collector`]
const DEFAULT_CACHE_THRESHOLD: u64 = 100_000;

/// Collects all history (`H`) indices for a range of changesets (`CS`) and stores them in a
/// [`Collector`].
///
/// ## Process
/// The function utilizes a [`HistoryIndexCache`] with a structure of `PartialKey` (`P`) (Address or
/// Address.StorageKey) to block numbers. When the cache exceeds its capacity, its contents are
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

    let mut cache =
        HistoryIndexCache::new(etl_config, HashMap::<_, _>::default(), sharded_key_factory);

    // observability
    let total_changesets = provider.tx_ref().entries::<CS>()?;
    let interval = (total_changesets / 1000).max(1);

    for (idx, entry) in changeset_cursor.walk_range(range)?.enumerate() {
        let (block_number, key) = partial_key_factory(entry?);
        cache.insert(block_number, key)?;

        if idx > 0 && idx.is_multiple_of(interval) && total_changesets > 1000 {
            info!(target: "sync::stages::index_history", progress = %format_args!("{:.4}%", (idx as f64 / total_changesets as f64) * 100.0), "Collecting indices");
        }
    }

    cache.finish()
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
    let mut cache = HistoryIndexCache::new(etl_config, AddressMap::default(), ShardedKey::new);

    // Convert range bounds to concrete range
    let range = to_range(range);
    let start_block = range.start;

    // Use the new walker for lazy iteration over static file changesets
    let static_file_provider = provider.static_file_provider();

    let walker = static_file_provider.walk_account_changeset_range(range);

    for changeset_result in walker {
        let (block_number, AccountBeforeTx { address, .. }) = changeset_result?;
        if cache.insert(block_number, address)? {
            info!(
                target: "sync::stages::index_history",
                processed_blocks = block_number.saturating_sub(start_block),
                current_block = block_number,
                "Collecting indices"
            );
        }
    }

    cache.finish()
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
    let mut cache = HistoryIndexCache::new(
        etl_config,
        HashMap::<_, _>::default(),
        |AddressStorageKey((address, storage_key)), highest_block_number| {
            StorageShardedKey::new(address, storage_key, highest_block_number)
        },
    );

    let range = to_range(range);
    let start_block = range.start;
    let static_file_provider = provider.static_file_provider();

    let walker = static_file_provider.walk_storage_changeset_range(range);

    for changeset_result in walker {
        let (BlockNumberAddress((block_number, address)), storage) = changeset_result?;
        if cache.insert(block_number, AddressStorageKey((address, storage.key)))? {
            info!(
                target: "sync::stages::index_history",
                processed_blocks = block_number.saturating_sub(start_block),
                current_block = block_number,
                "Collecting indices"
            );
        }
    }

    cache.finish()
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
            info!(target: "sync::stages::index_history", progress = %format_args!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
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
            info!(target: "sync::stages::index_history", progress = %format_args!("{:.2}%", (index as f64 / total_entries as f64) * 100.0), "Writing indices");
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
                    append_only,
                    writer,
                )?;
            }

            current_key = Some(partial_key);
            current_list.clear();

            // On incremental sync, merge with the existing last shard from the database.
            // The last shard is stored with key (address, storage_key, u64::MAX) so we can find it.
            if !append_only &&
                let Some(last_shard) =
                    writer.get_last_storage_history_shard(partial_key.0, partial_key.1)?
            {
                current_list.extend(last_shard.iter());
            }
        }

        // Append new block numbers to the accumulator.
        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last (partial) shard buffered.
        flush_storage_history_shards_partial(
            partial_key.0,
            partial_key.1,
            &mut current_list,
            append_only,
            writer,
        )?;
    }

    // Flush the final key's remaining shard.
    if let Some((addr, storage_key)) = current_key {
        flush_storage_history_shards(addr, storage_key, &mut current_list, append_only, writer)?;
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
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::StoragesHistory>
        + DbCursorRO<reth_db_api::tables::StoragesHistory>,
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

        if append_only {
            writer.append_storage_history(key, &value)?;
        } else {
            writer.upsert_storage_history(key, &value)?;
        }
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
    append_only: bool,
    writer: &mut EitherWriter<'_, CURSOR, N>,
) -> Result<(), StageError>
where
    N: NodePrimitives,
    CURSOR: DbCursorRW<reth_db_api::tables::StoragesHistory>
        + DbCursorRO<reth_db_api::tables::StoragesHistory>,
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

        if append_only {
            writer.append_storage_history(key, &value)?;
        } else {
            writer.upsert_storage_history(key, &value)?;
        }
    }

    list.clear();
    Ok(())
}

/// Buffers the block numbers of each history key before they move to an ETL [`Collector`].
///
/// The cache moves its entries to the collector at the first block boundary after
/// [`DEFAULT_CACHE_THRESHOLD`] blocks, or after its estimated heap use reaches half the ETL file
/// size, whichever comes first, so the cache does not grow with the number of distinct keys in the
/// block range. Entries move only at block boundaries, so two collector entries of one key never
/// end at the same block.
struct HistoryIndexCache<P, K: Key, F, S> {
    collector: Collector<K, BlockNumberList>,
    /// Creates the collector key from a partial key and its highest block number.
    sharded_key: F,
    indices: HashMap<P, Vec<u64>, S>,
    /// Estimated heap bytes held by `indices`.
    size_bytes: usize,
    /// Value of `size_bytes` at which the cache flushes on the next block boundary.
    max_size_bytes: usize,
    /// Number of blocks inserted since the last [`DEFAULT_CACHE_THRESHOLD`] flush.
    blocks: u64,
    /// Block number of the last insert.
    current_block: Option<BlockNumber>,
}

impl<P, K, F, S> HistoryIndexCache<P, K, F, S>
where
    P: Eq + Hash,
    K: Key,
    F: Fn(P, BlockNumber) -> K,
    S: BuildHasher,
{
    fn new(etl_config: &EtlConfig, indices: HashMap<P, Vec<u64>, S>, sharded_key: F) -> Self {
        Self {
            collector: Collector::new(etl_config.file_size, etl_config.dir.clone()),
            sharded_key,
            indices,
            size_bytes: 0,
            max_size_bytes: etl_config.file_size / 2,
            blocks: 0,
            current_block: None,
        }
    }

    /// Adds `block_number` to the indices of `key`.
    ///
    /// Block numbers must not decrease between calls. Returns `true` if the cache reached
    /// [`DEFAULT_CACHE_THRESHOLD`] blocks and flushed to the collector before the insert. A flush
    /// at the size bound does not reset the block count, so the `true` results keep a fixed cadence
    /// for progress logs.
    fn insert(&mut self, block_number: BlockNumber, key: P) -> Result<bool, StageError> {
        let mut flushed = false;
        if self.current_block != Some(block_number) {
            if self.blocks >= DEFAULT_CACHE_THRESHOLD {
                self.flush()?;
                self.blocks = 0;
                flushed = true;
            } else if self.size_bytes >= self.max_size_bytes {
                debug!(
                    target: "sync::stages::index_history",
                    current_block = block_number,
                    size_bytes = self.size_bytes,
                    "Flushing history index cache at size bound"
                );
                self.flush()?;
            }
            self.current_block = Some(block_number);
            self.blocks += 1;
        }

        let indices = self.indices.entry(key).or_default();
        let capacity = indices.capacity();
        if capacity == 0 {
            // A new key: its map slot plus one hashbrown control byte.
            self.size_bytes += size_of::<(P, Vec<u64>)>() + 1;
        }
        indices.push(block_number);
        self.size_bytes += (indices.capacity() - capacity) * size_of::<u64>();

        Ok(flushed)
    }

    /// Moves all buffered indices to the collector.
    fn flush(&mut self) -> Result<(), StageError> {
        for (key, indices) in self.indices.drain() {
            let last = *indices.last().expect("indices is non-empty");
            self.collector
                .insert((self.sharded_key)(key, last), BlockNumberList::new_pre_sorted(indices))?;
        }
        self.size_bytes = 0;
        Ok(())
    }

    /// Flushes the remaining indices and returns the collector.
    fn finish(mut self) -> Result<Collector<K, BlockNumberList>, StageError> {
        self.flush()?;
        Ok(self.collector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use reth_testing_utils::generators;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn history_index_cache_flushes_by_size() {
        const KEYS_PER_BLOCK: usize = 50;
        const BLOCKS: u64 = 2_000;

        let mut rng = generators::rng();
        let mut cache = HistoryIndexCache::new(
            &EtlConfig::new(None, 128 * 1024),
            AddressMap::default(),
            ShardedKey::new,
        );
        let mut history = BTreeMap::<Address, Vec<u64>>::new();
        let mut flushes = 0;
        for block in 0..BLOCKS {
            let addresses = (0..KEYS_PER_BLOCK)
                .map(|_| Address::left_padding_from(&rng.random_range(0..20_000u64).to_be_bytes()))
                .collect::<BTreeSet<_>>();
            for address in addresses {
                let size_bytes = cache.size_bytes;
                // The block threshold does not fire in `BLOCKS` blocks, so all flushes come from
                // the size bound.
                assert!(!cache.insert(block, address).unwrap());
                flushes += (cache.size_bytes < size_bytes) as usize;
                history.entry(address).or_default().push(block);
            }
        }
        assert!(flushes > 10, "cache flushed {flushes} times");

        // The collector entries of each key hold its blocks in order.
        let mut collector = cache.finish().unwrap();
        let mut collected = BTreeMap::<Address, Vec<u64>>::new();
        for entry in collector.iter().unwrap() {
            let (key, value) = entry.unwrap();
            let key = ShardedKey::<Address>::decode_owned(key).unwrap();
            let blocks =
                BlockNumberList::decompress_owned(value).unwrap().iter().collect::<Vec<_>>();
            assert_eq!(key.highest_block_number, *blocks.last().unwrap());
            collected.entry(key.key).or_default().extend(blocks);
        }
        assert_eq!(collected, history);
    }
}
