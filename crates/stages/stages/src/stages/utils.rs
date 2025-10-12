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
/// This function is the second phase of the history indexing process, taking the collected
/// indices from the ETL collector and organizing them into properly sharded database entries.
/// It handles the complex logic of merging existing shards with new data and managing
/// shard boundaries.
///
/// ## Process
///
/// 1. **Iteration**: Processes all entries from the collector in order
/// 2. **Partial Key Grouping**: Groups indices by their partial keys (e.g., `Address` or
///    `Address.StorageKey`)
/// 3. **Shard Management**: Flushes indices to disk when reaching shard limits or when partial key
///    changes
/// 4. **Existing Shard Merging**: For non-append-only mode, merges with existing database shards
/// 5. **Final Flush**: Ensures the last shard is properly stored
///
/// ## Key Logic
///
/// - **Shard Boundaries**: Automatically flushes when `NUM_OF_INDICES_IN_SHARD` is reached or when
///   the partial key changes
/// - **Merge Strategy**: In non-append-only mode, loads existing shards from the database and
///   merges them with new data before writing
/// - **Memory Efficiency**: Processes data incrementally to avoid loading entire datasets into
///   memory
/// - **Progress Tracking**: Provides progress updates for long-running operations
///
/// ## Parameters
///
/// - `provider`: Database provider for read/write operations
/// - `collector`: ETL collector containing the gathered history indices
/// - `append_only`: If true, only appends new data; if false, merges with existing shards
/// - `sharded_key_factory`: Function to create sharded keys from partial key and block number
/// - `decode_key`: Function to decode raw key bytes into typed keys
/// - `get_partial`: Function to extract partial key from a full sharded key
///
/// ## Returns
///
/// Returns `Ok(())` on success, or a [`StageError`] if database operations fail.
///
/// ## Example
///
/// For an address with indices `[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]` and `NUM_OF_INDICES_IN_SHARD = 3`:
/// - Shard 1: `(address, 3)` → `[1, 2, 3]`
/// - Shard 2: `(address, 6)` → `[4, 5, 6]`
/// - Shard 3: `(address, 9)` → `[7, 8, 9]`
/// - Shard 4: `(address, u64::MAX)` → `[10]` (final shard)
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

            // Switch to the new partial key and reset the current list
            current_partial = partial_key;
            current_list.clear();

            // If it's not the first sync, there might an existing shard already, so we need to
            // merge it with the one coming from the collector. This is crucial for incremental
            // updates where we need to combine new data with existing database entries.
            if !append_only &&
                let Some((_, last_database_shard)) =
                    write_cursor.seek_exact(sharded_key_factory(current_partial, u64::MAX))?
            {
                // Load existing indices from the database and merge them with new data
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
///
/// This function handles the complex logic of sharding large index lists into smaller chunks
/// that fit within the database's shard size limits. It's a critical component of the history
/// indexing system that ensures efficient storage and retrieval of block number lists.
///
/// ## Process
///
/// 1. **Sharding Decision**: Only processes the list if it exceeds `NUM_OF_INDICES_IN_SHARD` or if
///    the mode requires flushing all data
/// 2. **Chunking**: Splits the list into chunks of maximum size `NUM_OF_INDICES_IN_SHARD`
/// 3. **Key Generation**: Creates sharded keys using the highest block number in each chunk
/// 4. **Storage Strategy**: Uses either append-only or upsert operations based on the `append_only`
///    flag
/// 5. **Last Chunk Handling**: Keeps the last chunk in memory if not flushing, otherwise stores it
///
/// ## Key Logic
///
/// - **Shard Key Strategy**: Uses the highest block number in each chunk as the shard key
/// - **Last Shard Special Case**: The final shard uses `u64::MAX` as its key to ensure it's always
///   the last shard for a given partial key
/// - **Memory Management**: When not flushing, the last chunk is kept in the input `list` for
///   potential future merging with additional data
///
/// ## Example
///
/// For a list `[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]` with `NUM_OF_INDICES_IN_SHARD = 3`:
/// - Chunk 1: `[1, 2, 3]` → key: `(partial_key, 3)`
/// - Chunk 2: `[4, 5, 6]` → key: `(partial_key, 6)`
/// - Chunk 3: `[7, 8, 9]` → key: `(partial_key, 9)`
/// - Chunk 4: `[10]` → key: `(partial_key, u64::MAX)` (if flushing)
///
/// ## Parameters
///
/// - `cursor`: Database cursor for read/write operations
/// - `partial_key`: The base key (e.g., address or storage key) for this index set
/// - `list`: Mutable reference to the block number list to be sharded
/// - `sharded_key_factory`: Function to create sharded keys from partial key and block number
/// - `append_only`: If true, uses append operations; otherwise uses upsert
/// - `mode`: Controls whether to flush all chunks or keep the last one in memory
///
/// ## Returns
///
/// Returns `Ok(())` on success, or a [`StageError`] if database operations fail.
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
    // Only process if the list exceeds shard size or we need to flush everything
    if list.len() > NUM_OF_INDICES_IN_SHARD || mode.is_flush() {
        // Split the list into chunks of maximum shard size
        let chunks = list
            .chunks(NUM_OF_INDICES_IN_SHARD)
            .map(|chunks| chunks.to_vec())
            .collect::<Vec<Vec<u64>>>();

        let mut iter = chunks.into_iter().peekable();
        while let Some(chunk) = iter.next() {
            let mut highest = *chunk.last().expect("at least one index");

            // Handle the last chunk specially based on flush mode
            if !mode.is_flush() && iter.peek().is_none() {
                // Keep the last chunk in memory for potential future merging
                *list = chunk;
            } else {
                // For the final chunk when flushing, use u64::MAX as the key
                // This ensures it's always the last shard for this partial key
                if iter.peek().is_none() {
                    highest = u64::MAX;
                }

                // Create the sharded key and store the chunk
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
///
/// This function handles the critical case where the database contains more transaction data than
/// what's available in the static files. This can happen during sync operations when the database
/// has been updated but the corresponding static files haven't been generated yet.
///
/// ## Process
///
/// 1. **Find Highest Static Block**: Gets the highest block number available in static files
/// 2. **Validate Transaction Alignment**: Ensures the last transaction number in the static files
///    doesn't exceed the expected `last_tx_num`
/// 3. **Backtrack if Necessary**: If there's a mismatch, walks backwards through blocks to find the
///    last valid block where transaction indices are consistent
/// 4. **Identify Missing Block**: The missing block is the next block after the last valid one
/// 5. **Create Error**: Returns a `MissingStaticFileData` error with the missing block information
///
/// ## Key Logic
///
/// - **Safety Check**: The function performs a safety validation to ensure transaction number
///   consistency between the database and static files
/// - **Backtracking Strategy**: If inconsistencies are found, it walks backwards block by block
///   until it finds a block where `indices.last_tx_num() <= last_tx_num`
/// - **Boundary Handling**: Stops at block 0 to prevent infinite loops
/// - **Error Construction**: Creates a comprehensive error that includes both the missing block and
///   the affected segment for proper error reporting
///
/// ## Parameters
///
/// - `last_tx_num`: The last transaction number expected in the database
/// - `static_file_provider`: Provider for accessing static file data
/// - `provider`: Main database provider for reading block and transaction data
/// - `segment`: The static file segment that's missing data
///
/// ## Returns
///
/// Returns a [`StageError::MissingStaticFileData`] containing the missing block information,
/// or a [`ProviderError`] if database operations fail.
///
/// ## Example
///
/// If the database has transactions up to tx_num 1000, but static files only contain data
/// up to block 50 with tx_num 800, this function will:
/// 1. Find block 50 as the highest static file block
/// 2. Verify that block 50's last tx_num (800) <= 1000 ✓
/// 3. Identify block 51 as the first missing block
/// 4. Return an error indicating block 51 is missing from static files
pub(crate) fn missing_static_data_error<Provider>(
    last_tx_num: TxNumber,
    static_file_provider: &StaticFileProvider<Provider::Primitives>,
    provider: &Provider,
    segment: StaticFileSegment,
) -> Result<StageError, ProviderError>
where
    Provider: BlockReader + StaticFileProviderFactory,
{
    // Start with the highest block available in static files
    let mut last_block =
        static_file_provider.get_highest_static_file_block(segment).unwrap_or_default();

    // Safety check: ensure the last transaction number in static files doesn't exceed
    // the expected last transaction number. If it does, we need to find the correct
    // last block by walking backwards.
    loop {
        if let Some(indices) = provider.block_body_indices(last_block)? &&
            indices.last_tx_num() <= last_tx_num
        {
            // Found a valid block where transaction indices are consistent
            break
        }
        if last_block == 0 {
            // Reached the genesis block, stop to prevent infinite loop
            break
        }
        // Walk backwards to find the last consistent block
        last_block -= 1;
    }

    // The missing block is the next one after the last valid block
    let missing_block = Box::new(provider.sealed_header(last_block + 1)?.unwrap_or_default());

    Ok(StageError::MissingStaticFileData {
        block: Box::new(missing_block.block_with_parent()),
        segment,
    })
}
