//! Utils for `stages`.
use alloy_primitives::{map::HashMap, Address, BlockNumber, TxNumber, B256};
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
use std::{hash::Hash, mem::size_of, ops::RangeBounds};
use tracing::info;

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

    let mut cache = HistoryIndexCache::new(etl_config, sharded_key_factory);

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
    let mut cache = HistoryIndexCache::new(etl_config, ShardedKey::new);

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
/// [`DEFAULT_CACHE_THRESHOLD`] blocks, or after its estimated heap use reaches the ETL file size,
/// whichever comes first. The size bound keeps memory independent of the number of distinct keys
/// in the block range. Entries move only at block boundaries, so two collector entries of one key
/// never end at the same block.
struct HistoryIndexCache<P, K: Key, F> {
    collector: Collector<K, BlockNumberList>,
    /// Creates the collector key from a partial key and its highest block number.
    sharded_key: F,
    indices: HashMap<P, Vec<u64>>,
    /// Estimated heap bytes held by `indices`.
    size_bytes: usize,
    /// Value of `size_bytes` at which the cache flushes on the next block boundary.
    max_size_bytes: usize,
    /// Number of blocks inserted since the last flush.
    blocks: u64,
    /// Block number of the last insert.
    current_block: Option<BlockNumber>,
}

impl<P, K, F> HistoryIndexCache<P, K, F>
where
    P: Eq + Hash,
    K: Key,
    F: Fn(P, BlockNumber) -> K,
{
    fn new(etl_config: &EtlConfig, sharded_key: F) -> Self {
        Self {
            collector: Collector::new(etl_config.file_size, etl_config.dir.clone()),
            sharded_key,
            indices: HashMap::default(),
            size_bytes: 0,
            max_size_bytes: etl_config.file_size,
            blocks: 0,
            current_block: None,
        }
    }

    /// Adds `block_number` to the indices of `key`.
    ///
    /// Block numbers must not decrease between calls. Returns `true` if the cache flushed to the
    /// collector before the insert.
    fn insert(&mut self, block_number: BlockNumber, key: P) -> Result<bool, StageError> {
        let mut flushed = false;
        if self.current_block != Some(block_number) {
            if self.blocks >= DEFAULT_CACHE_THRESHOLD || self.size_bytes >= self.max_size_bytes {
                self.flush()?;
                flushed = true;
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
        self.blocks = 0;
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
    use crate::{
        stages::{IndexAccountHistoryStage, IndexStorageHistoryStage},
        test_utils::TestStageDB,
    };
    use alloy_primitives::U256;
    use rand::Rng;
    use reth_db_api::{
        models::{StorageBeforeTx, StoredBlockBodyIndices},
        table::{Compress, IntoVec},
        tables::{self, RawTable},
        transaction::DbTxMut,
    };
    use reth_primitives_traits::StorageEntry;
    use reth_provider::{
        providers::StaticFileWriter, DatabaseProviderFactory, RocksDBProviderFactory,
        StorageSettingsCache,
    };
    use reth_stages_api::{ExecInput, ExecOutput, Stage, StageCheckpoint};
    use reth_storage_api::StorageSettings;
    use reth_testing_utils::generators;
    use std::collections::{BTreeMap, BTreeSet};

    /// Last block of the generated changesets.
    const LAST_BLOCK: BlockNumber = 4_500;

    /// ETL file size that makes the cache flush every few hundred blocks of the generated
    /// changesets.
    const SMALL_ETL_FILE_SIZE: usize = 256 * 1024;

    /// Changed `(address, slot)` pairs per block. Three hot addresses change in every block, so
    /// their history spans several shards. The other pairs repeat across blocks.
    fn generate_changes() -> Vec<BTreeSet<(Address, B256)>> {
        let mut rng = generators::rng();
        (0..=LAST_BLOCK)
            .map(|_| {
                let mut changes = BTreeSet::new();
                for hot in 250..=252 {
                    for slot in 0..2 {
                        changes.insert((Address::with_last_byte(hot), B256::with_last_byte(slot)));
                    }
                }
                for _ in 0..15 {
                    changes.insert((
                        Address::with_last_byte(rng.random()),
                        B256::with_last_byte(rng.random_range(0..8)),
                    ));
                }
                changes
            })
            .collect()
    }

    /// Writes `changes` as v1 (MDBX) or v2 (static file) changesets.
    fn setup(changes: &[BTreeSet<(Address, B256)>], v2: bool) -> TestStageDB {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(if v2 {
            StorageSettings::v2()
        } else {
            StorageSettings::v1()
        });

        db.commit(|tx| {
            for (block, block_changes) in changes.iter().enumerate() {
                let block = block as BlockNumber;
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                )?;
                if v2 {
                    continue
                }
                let addresses =
                    block_changes.iter().map(|(address, _)| *address).collect::<BTreeSet<_>>();
                for address in addresses {
                    tx.put::<tables::AccountChangeSets>(
                        block,
                        AccountBeforeTx { address, info: None },
                    )?;
                }
                for &(address, key) in block_changes {
                    tx.put::<tables::StorageChangeSets>(
                        BlockNumberAddress((block, address)),
                        StorageEntry { key, value: U256::ZERO },
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();

        if v2 {
            let static_file_provider = db.factory.static_file_provider();
            let mut account_writer =
                static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            let mut storage_writer =
                static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            for (block, block_changes) in changes.iter().enumerate() {
                let block = block as BlockNumber;
                let addresses =
                    block_changes.iter().map(|(address, _)| *address).collect::<BTreeSet<_>>();
                account_writer
                    .append_account_changeset(
                        addresses
                            .into_iter()
                            .map(|address| AccountBeforeTx { address, info: None })
                            .collect(),
                        block,
                    )
                    .unwrap();
                storage_writer
                    .append_storage_changeset(
                        block_changes
                            .iter()
                            .map(|&(address, key)| StorageBeforeTx {
                                address,
                                key,
                                value: U256::ZERO,
                            })
                            .collect(),
                        block,
                    )
                    .unwrap();
            }
            account_writer.commit().unwrap();
            storage_writer.commit().unwrap();
        }

        db
    }

    /// Raw history shards that the stages must write for `history`: the blocks of each key cut
    /// into shards of [`NUM_OF_INDICES_IN_SHARD`], with the last shard keyed by `u64::MAX`.
    fn expected_shards<P: Copy, K: Key>(
        history: BTreeMap<P, Vec<u64>>,
        sharded_key: impl Fn(P, BlockNumber) -> K,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut shards = Vec::new();
        for (key, blocks) in history {
            let chunks = blocks.chunks(NUM_OF_INDICES_IN_SHARD).collect::<Vec<_>>();
            for (i, chunk) in chunks.iter().enumerate() {
                let highest = if i == chunks.len() - 1 { u64::MAX } else { *chunk.last().unwrap() };
                shards.push((
                    sharded_key(key, highest).encode().into_vec(),
                    BlockNumberList::new_pre_sorted(chunk.iter().copied()).compress(),
                ));
            }
        }
        shards.sort();
        shards
    }

    fn expected_account_shards(
        changes: &[BTreeSet<(Address, B256)>],
        last_block: BlockNumber,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut history = BTreeMap::<_, Vec<_>>::new();
        for (block, block_changes) in changes[..=last_block as usize].iter().enumerate() {
            for address in
                block_changes.iter().map(|(address, _)| *address).collect::<BTreeSet<_>>()
            {
                history.entry(address).or_default().push(block as BlockNumber);
            }
        }
        expected_shards(history, ShardedKey::new)
    }

    fn expected_storage_shards(
        changes: &[BTreeSet<(Address, B256)>],
        last_block: BlockNumber,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut history = BTreeMap::<_, Vec<_>>::new();
        for (block, block_changes) in changes[..=last_block as usize].iter().enumerate() {
            for &key in block_changes {
                history.entry(key).or_default().push(block as BlockNumber);
            }
        }
        expected_shards(history, |(address, key), highest| {
            StorageShardedKey::new(address, key, highest)
        })
    }

    /// Reads all raw shards of history table `T` from MDBX (v1) or `RocksDB` (v2).
    fn raw_shards<T: Table>(db: &TestStageDB, v2: bool) -> Vec<(Vec<u8>, Vec<u8>)> {
        if v2 {
            db.factory
                .rocksdb_provider()
                .raw_iter::<T>()
                .unwrap()
                .map(|entry| {
                    let (key, value) = entry.unwrap();
                    (key.into_vec(), value.into_vec())
                })
                .collect()
        } else {
            db.query(|tx| {
                Ok(tx
                    .cursor_read::<RawTable<T>>()?
                    .walk(None)?
                    .map(|entry| {
                        let (key, value) = entry.unwrap();
                        (key.into_key(), value.raw_value().to_vec())
                    })
                    .collect())
            })
            .unwrap()
        }
    }

    /// Runs the account or storage history stage from `checkpoint` to `target`.
    fn run(
        db: &TestStageDB,
        storage: bool,
        checkpoint: Option<BlockNumber>,
        target: BlockNumber,
        etl_file_size: usize,
    ) {
        let input =
            ExecInput { target: Some(target), checkpoint: checkpoint.map(StageCheckpoint::new) };
        let etl_config = EtlConfig::new(None, etl_file_size);
        let provider = db.factory.database_provider_rw().unwrap();
        let out = if storage {
            IndexStorageHistoryStage { etl_config, ..Default::default() }.execute(&provider, input)
        } else {
            IndexAccountHistoryStage { etl_config, ..Default::default() }.execute(&provider, input)
        };
        assert_eq!(out.unwrap(), ExecOutput::done(StageCheckpoint::new(target)));
        provider.commit().unwrap();
    }

    /// Runs a history stage over the generated changesets: a first sync to block 50, incremental
    /// syncs to blocks 2500 and [`LAST_BLOCK`] that append to existing shards, and a first sync of
    /// the whole range on a fresh database. The cache flushes many times in the large ranges.
    /// After each run, the written shards must match [`expected_shards`] byte for byte.
    fn check_history_shards(storage: bool, v2: bool) {
        let changes = generate_changes();
        let expected = |last_block| {
            if storage {
                expected_storage_shards(&changes, last_block)
            } else {
                expected_account_shards(&changes, last_block)
            }
        };
        let actual = |db: &TestStageDB| {
            if storage {
                raw_shards::<tables::StoragesHistory>(db, v2)
            } else {
                raw_shards::<tables::AccountsHistory>(db, v2)
            }
        };
        let default_file_size = EtlConfig::default_file_size();

        let db = setup(&changes, v2);
        run(&db, storage, None, 50, default_file_size);
        assert_eq!(actual(&db), expected(50));
        run(&db, storage, Some(50), 2_500, SMALL_ETL_FILE_SIZE);
        assert_eq!(actual(&db), expected(2_500));
        run(&db, storage, Some(2_500), LAST_BLOCK, SMALL_ETL_FILE_SIZE);
        assert_eq!(actual(&db), expected(LAST_BLOCK));

        let db = setup(&changes, v2);
        run(&db, storage, None, LAST_BLOCK, SMALL_ETL_FILE_SIZE);
        assert_eq!(actual(&db), expected(LAST_BLOCK));
    }

    #[test]
    fn account_history_shards_v1() {
        check_history_shards(false, false);
    }

    #[test]
    fn account_history_shards_v2() {
        check_history_shards(false, true);
    }

    #[test]
    fn storage_history_shards_v1() {
        check_history_shards(true, false);
    }

    #[test]
    fn storage_history_shards_v2() {
        check_history_shards(true, true);
    }

    #[test]
    fn history_index_cache_stays_under_size_bound() {
        const KEYS_PER_BLOCK: usize = 50;
        const BLOCKS: u64 = 2_000;
        let max_size_bytes = 64 * 1024;

        let mut rng = generators::rng();
        let mut cache =
            HistoryIndexCache::new(&EtlConfig::new(None, max_size_bytes), ShardedKey::new);
        // Entries move to the collector only at block boundaries, so the cache can exceed the
        // bound by the indices of one block: a new key and a grown `Vec` for each change.
        let max_block_bytes =
            KEYS_PER_BLOCK * (size_of::<(Address, Vec<u64>)>() + 1 + 4 * size_of::<u64>());
        let mut history = BTreeMap::<Address, Vec<u64>>::new();
        let mut flushes = 0;
        for block in 0..BLOCKS {
            let addresses = (0..KEYS_PER_BLOCK)
                .map(|_| Address::left_padding_from(&rng.random_range(0..20_000u64).to_be_bytes()))
                .collect::<BTreeSet<_>>();
            for address in addresses {
                flushes += cache.insert(block, address).unwrap() as usize;
                assert!(cache.size_bytes < max_size_bytes + max_block_bytes);
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
