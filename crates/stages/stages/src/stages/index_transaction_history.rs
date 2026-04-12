use alloy_consensus::Transaction;
use alloy_primitives::{map::AddressMap, Address, BlockNumber};
use reth_config::config::{EtlConfig, IndexHistoryConfig};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, ShardedKey},
    table::{Decode, Decompress, Table},
    tables,
    transaction::DbTxMut,
    BlockNumberList,
};
use reth_etl::Collector;
use reth_provider::{
    BlockReader, DBProvider, StaticFileProviderFactory, StorageSettingsCache, TransactionsProvider,
};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use std::fmt::Debug;
use tracing::info;

/// Stage that indexes transaction senders and recipients by block number.
///
/// Reads from [`tables::TransactionSenders`] and [`tables::Transactions`] to build
/// [`tables::CallTraceFromIndex`] and [`tables::CallTraceToIndex`], which map addresses
/// to the block numbers where they appeared as transaction sender or recipient.
///
/// This enables `trace_filter` to quickly narrow down candidate blocks for an address
/// without re-executing every block in the requested range.
#[derive(Debug)]
pub struct IndexTransactionHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl IndexTransactionHistoryStage {
    /// Create new instance of [`IndexTransactionHistoryStage`].
    pub const fn new(config: IndexHistoryConfig, etl_config: EtlConfig) -> Self {
        Self { commit_threshold: config.commit_threshold, etl_config }
    }
}

impl Default for IndexTransactionHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000, etl_config: EtlConfig::default() }
    }
}

impl<Provider> Stage<Provider> for IndexTransactionHistoryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + BlockReader
        + TransactionsProvider
        + StaticFileProviderFactory
        + StorageSettingsCache,
{
    fn id(&self) -> StageId {
        StageId::IndexTransactionHistory
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let mut range = input.next_block_range();
        let first_sync = input.checkpoint().block_number == 0;

        // On first sync, clear the tables since it's faster to rebuild from scratch.
        if first_sync {
            provider.tx_ref().clear::<tables::CallTraceFromIndex>()?;
            provider.tx_ref().clear::<tables::CallTraceToIndex>()?;
            range = 0..=*input.next_block_range().end();
        }

        info!(target: "sync::stages::index_transaction_history::exec", ?first_sync, ?range, "Collecting transaction history indices");

        let (from_collector, to_collector) =
            collect_transaction_history_indices(provider, range.clone(), &self.etl_config)?;

        info!(target: "sync::stages::index_transaction_history::exec", "Loading indices into database");

        load_call_trace_index(
            from_collector,
            first_sync,
            &mut provider.tx_ref().cursor_write::<tables::CallTraceFromIndex>()?,
        )?;

        load_call_trace_index(
            to_collector,
            first_sync,
            &mut provider.tx_ref().cursor_write::<tables::CallTraceToIndex>()?,
        )?;

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(*range.end()), done: true })
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        unwind_transaction_history_indices(provider, range)?;

        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_progress) })
    }
}

/// Collects transaction history indices for both sender and recipient addresses.
///
/// A pair of ETL collectors for from/to address call trace indices.
type CallTraceCollectors = (
    Collector<ShardedKey<Address>, BlockNumberList>,
    Collector<ShardedKey<Address>, BlockNumberList>,
);

/// Iterates over blocks in the given range, reads their transactions and senders,
/// and groups addresses by block number into two ETL collectors (from/to).
fn collect_transaction_history_indices<Provider>(
    provider: &Provider,
    range: std::ops::RangeInclusive<BlockNumber>,
    etl_config: &EtlConfig,
) -> Result<CallTraceCollectors, StageError>
where
    Provider: BlockReader + TransactionsProvider,
{
    let mut from_collector = Collector::new(etl_config.file_size, etl_config.dir.clone());
    let mut to_collector = Collector::new(etl_config.file_size, etl_config.dir.clone());

    let mut from_cache: AddressMap<Vec<u64>> = AddressMap::default();
    let mut to_cache: AddressMap<Vec<u64>> = AddressMap::default();

    let total_blocks = range.end().saturating_sub(*range.start()) + 1;
    let interval = (total_blocks / 1000).max(1);

    /// Maximum number of address-block entries to buffer before flushing to the ETL collector.
    /// This bounds memory usage regardless of how many blocks are processed.
    const MAX_CACHE_ENTRIES: usize = 1_000_000;

    let mut cache_entries = 0usize;

    for (idx, block_number) in range.enumerate() {
        let body_indices = provider.block_body_indices(block_number)?.ok_or(
            StageError::MissingStaticFileData {
                block: Box::new(alloy_eips::eip1898::BlockWithParent {
                    parent: Default::default(),
                    block: alloy_eips::eip1898::BlockNumHash::new(block_number, Default::default()),
                }),
                segment: reth_static_file_types::StaticFileSegment::Transactions,
            },
        )?;

        if body_indices.tx_count == 0 {
            continue;
        }

        let tx_range = body_indices.tx_num_range();

        let senders = provider.senders_by_tx_range(tx_range.clone())?;
        let transactions = provider.transactions_by_tx_range(tx_range)?;

        for (sender, tx) in senders.into_iter().zip(transactions.iter()) {
            from_cache.entry(sender).or_default().push(block_number);
            cache_entries += 1;

            if let Some(to_addr) = tx.to() {
                to_cache.entry(to_addr).or_default().push(block_number);
                cache_entries += 1;
            }
        }

        if idx > 0 && (idx as u64).is_multiple_of(interval) && total_blocks > 1000 {
            info!(
                target: "sync::stages::index_transaction_history",
                progress = %format!("{:.4}%", (idx as f64 / total_blocks as f64) * 100.0),
                "Collecting indices"
            );
        }

        if cache_entries > MAX_CACHE_ENTRIES {
            flush_cache(&mut from_cache, &mut from_collector)?;
            flush_cache(&mut to_cache, &mut to_collector)?;
            cache_entries = 0;
        }
    }

    // Flush remaining entries
    flush_cache(&mut from_cache, &mut from_collector)?;
    flush_cache(&mut to_cache, &mut to_collector)?;

    Ok((from_collector, to_collector))
}

/// Flushes the address->block_numbers cache into a collector.
fn flush_cache(
    cache: &mut AddressMap<Vec<u64>>,
    collector: &mut Collector<ShardedKey<Address>, BlockNumberList>,
) -> Result<(), StageError> {
    for (address, mut indices) in cache.drain() {
        indices.dedup();
        let Some(&last) = indices.last() else { continue };
        collector
            .insert(ShardedKey::new(address, last), BlockNumberList::new_pre_sorted(indices))?;
    }
    Ok(())
}

/// Loads collected call trace indices into a database table.
///
/// Generic over the table type so it works for both [`tables::CallTraceFromIndex`] and
/// [`tables::CallTraceToIndex`].
fn load_call_trace_index<T, C>(
    mut collector: Collector<ShardedKey<Address>, BlockNumberList>,
    append_only: bool,
    cursor: &mut C,
) -> Result<(), StageError>
where
    T: Table<Key = ShardedKey<Address>, Value = BlockNumberList>,
    C: DbCursorRW<T> + DbCursorRO<T>,
{
    let mut current_address: Option<Address> = None;
    let mut current_list = Vec::<u64>::new();

    let total_entries = collector.len();
    let interval = (total_entries / 10).max(1);

    for (index, element) in collector.iter()?.enumerate() {
        let (k, v) = element?;
        let sharded_key = ShardedKey::<Address>::decode_owned(k)?;
        let new_list = BlockNumberList::decompress_owned(v)?;

        if index > 0 && index.is_multiple_of(interval) && total_entries > 10 {
            info!(
                target: "sync::stages::index_transaction_history",
                progress = %format!("{:.2}%", (index as f64 / total_entries as f64) * 100.0),
                "Writing indices"
            );
        }

        if current_address != Some(sharded_key.key) {
            // Flush the previous address's shards
            if let Some(prev_addr) = current_address {
                flush_shards::<T, C>(prev_addr, &mut current_list, append_only, cursor)?;
            }

            current_address = Some(sharded_key.key);
            current_list.clear();

            // On incremental sync, merge with existing last shard
            if !append_only &&
                let Some(existing) =
                    cursor.seek_exact(ShardedKey::last(sharded_key.key))?.map(|(_, v)| v)
            {
                current_list.extend(existing.iter());
            }
        }

        current_list.extend(new_list.iter());

        // Flush complete shards, keeping the last partial one buffered
        if current_list.len() > NUM_OF_INDICES_IN_SHARD {
            let num_full = current_list.len() / NUM_OF_INDICES_IN_SHARD;
            let shards_to_flush = if current_list.len().is_multiple_of(NUM_OF_INDICES_IN_SHARD) {
                num_full - 1
            } else {
                num_full
            };

            if shards_to_flush > 0 {
                let flush_len = shards_to_flush * NUM_OF_INDICES_IN_SHARD;
                let remainder = current_list.split_off(flush_len);
                if let Some(addr) = current_address {
                    for chunk in current_list.chunks(NUM_OF_INDICES_IN_SHARD) {
                        let Some(&highest) = chunk.last() else { continue };
                        let key = ShardedKey::new(addr, highest);
                        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());
                        if append_only {
                            cursor.append(key, &value)?;
                        } else {
                            cursor.upsert(key, &value)?;
                        }
                    }
                }
                current_list = remainder;
            }
        }
    }

    // Flush the final address's remaining shard
    if let Some(addr) = current_address {
        flush_shards::<T, C>(addr, &mut current_list, append_only, cursor)?;
    }

    Ok(())
}

/// Flushes all remaining shards for an address, using `u64::MAX` for the last shard.
fn flush_shards<T, C>(
    address: Address,
    list: &mut Vec<u64>,
    append_only: bool,
    cursor: &mut C,
) -> Result<(), StageError>
where
    T: Table<Key = ShardedKey<Address>, Value = BlockNumberList>,
    C: DbCursorRW<T> + DbCursorRO<T>,
{
    if list.is_empty() {
        return Ok(());
    }

    let num_chunks = list.len().div_ceil(NUM_OF_INDICES_IN_SHARD);

    for (i, chunk) in list.chunks(NUM_OF_INDICES_IN_SHARD).enumerate() {
        let is_last = i == num_chunks - 1;
        let highest = if is_last { u64::MAX } else { chunk.last().copied().unwrap_or(u64::MAX) };
        let key = ShardedKey::new(address, highest);
        let value = BlockNumberList::new_pre_sorted(chunk.iter().copied());

        if append_only {
            cursor.append(key, &value)?;
        } else {
            cursor.upsert(key, &value)?;
        }
    }

    list.clear();
    Ok(())
}

/// Unwinds history shards for a given key, removing all block numbers at or above `block_number`.
///
/// Returns the remaining block numbers (below the unwind point) from the boundary shard
/// for reinsertion.
fn unwind_history_shards<T, C>(
    cursor: &mut C,
    start_key: T::Key,
    block_number: BlockNumber,
    mut shard_belongs_to_key: impl FnMut(&T::Key) -> bool,
) -> Result<Vec<u64>, StageError>
where
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<Address>>,
    C: DbCursorRO<T> + DbCursorRW<T>,
{
    let mut item = cursor.seek_exact(start_key)?;
    while let Some((sharded_key, list)) = item {
        if !shard_belongs_to_key(&sharded_key) {
            break
        }

        cursor.delete_current()?;

        let Some(first) = list.iter().next() else {
            item = cursor.prev()?;
            continue
        };

        if first >= block_number {
            item = cursor.prev()?;
            continue
        } else if block_number <= sharded_key.as_ref().highest_block_number {
            return Ok(list.iter().take_while(|i| *i < block_number).collect::<Vec<_>>())
        }
        return Ok(list.iter().collect::<Vec<_>>())
    }

    Ok(Vec::new())
}

/// Unwinds transaction history indices by removing block entries above the unwind target.
fn unwind_transaction_history_indices<Provider>(
    provider: &Provider,
    range: std::ops::RangeInclusive<BlockNumber>,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut> + BlockReader + TransactionsProvider,
{
    // Collect all addresses that need unwinding from the given block range
    let mut from_addresses: AddressMap<Vec<u64>> = AddressMap::default();
    let mut to_addresses: AddressMap<Vec<u64>> = AddressMap::default();

    for block_number in range {
        let body_indices = match provider.block_body_indices(block_number)? {
            Some(indices) => indices,
            None => continue,
        };

        if body_indices.tx_count == 0 {
            continue;
        }

        let tx_range = body_indices.tx_num_range();
        let senders = provider.senders_by_tx_range(tx_range.clone())?;
        let transactions = provider.transactions_by_tx_range(tx_range)?;

        for (sender, tx) in senders.into_iter().zip(transactions.iter()) {
            from_addresses.entry(sender).or_default().push(block_number);
            if let Some(to_addr) = tx.to() {
                to_addresses.entry(to_addr).or_default().push(block_number);
            }
        }
    }

    // Unwind from_index
    {
        let mut cursor = provider.tx_ref().cursor_write::<tables::CallTraceFromIndex>()?;
        for (address, indices) in &from_addresses {
            // Indices are sorted in block order; unwinding from the smallest block number
            // removes all entries >= that block, making subsequent calls redundant.
            if let Some(&min_block) = indices.first() {
                let partial_shard = unwind_history_shards::<tables::CallTraceFromIndex, _>(
                    &mut cursor,
                    ShardedKey::last(*address),
                    min_block,
                    |sharded_key| sharded_key.key == *address,
                )?;

                if !partial_shard.is_empty() {
                    cursor.insert(
                        ShardedKey::last(*address),
                        &BlockNumberList::new_pre_sorted(partial_shard),
                    )?;
                }
            }
        }
    }

    // Unwind to_index
    {
        let mut cursor = provider.tx_ref().cursor_write::<tables::CallTraceToIndex>()?;
        for (address, indices) in &to_addresses {
            if let Some(&min_block) = indices.first() {
                let partial_shard = unwind_history_shards::<tables::CallTraceToIndex, _>(
                    &mut cursor,
                    ShardedKey::last(*address),
                    min_block,
                    |sharded_key| sharded_key.key == *address,
                )?;

                if !partial_shard.is_empty() {
                    cursor.insert(
                        ShardedKey::last(*address),
                        &BlockNumberList::new_pre_sorted(partial_shard),
                    )?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestStageDB;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_primitives::{address, Address, Signature, TxKind};
    use reth_db_api::{
        models::{sharded_key::NUM_OF_INDICES_IN_SHARD, StoredBlockBodyIndices},
        tables,
        transaction::DbTxMut,
    };
    use reth_provider::{providers::StaticFileWriter, DatabaseProviderFactory};
    use reth_stages_api::{
        ExecInput, ExecOutput, Stage, StageCheckpoint, UnwindInput, UnwindOutput,
    };
    use reth_static_file_types::StaticFileSegment;
    use std::collections::BTreeMap;

    const SENDER_A: Address = address!("0x000000000000000000000000000000000000000a");
    const SENDER_B: Address = address!("0x000000000000000000000000000000000000000b");
    const RECIPIENT_X: Address = address!("0x0000000000000000000000000000000000000011");
    const RECIPIENT_Y: Address = address!("0x0000000000000000000000000000000000000022");

    type TransactionSigned = reth_ethereum_primitives::TransactionSigned;

    fn make_tx(to: TxKind) -> TransactionSigned {
        TxLegacy { to, ..Default::default() }.into_signed(Signature::test_signature()).into()
    }

    fn shard(addr: Address, highest: u64) -> ShardedKey<Address> {
        ShardedKey { key: addr, highest_block_number: highest }
    }

    fn cast(
        table: Vec<(ShardedKey<Address>, BlockNumberList)>,
    ) -> BTreeMap<ShardedKey<Address>, Vec<u64>> {
        table.into_iter().map(|(k, v)| (k, v.iter().collect())).collect()
    }

    /// Write block body indices, transaction senders (MDBX) and transactions (static files).
    fn setup_test_data(db: &TestStageDB, blocks: &[(u64, Vec<(Address, TxKind)>)]) {
        // Write transactions to static files first, then drop the writer before MDBX commit.
        {
            let provider = db.factory.static_file_provider();
            let mut txs_writer = provider.latest_writer(StaticFileSegment::Transactions).unwrap();

            let mut tx_num = 0u64;
            for &(block_number, ref txs) in blocks {
                // Backfill block gaps for static files (they require no gaps from block 0).
                let segment_header = txs_writer.user_header();
                if segment_header.block_end().is_none() &&
                    segment_header.expected_block_start() == 0
                {
                    for b in 0..block_number {
                        txs_writer.increment_block(b).unwrap();
                    }
                }

                for (_sender, to_kind) in txs {
                    let signed_tx = make_tx(*to_kind);
                    txs_writer.append_transaction(tx_num, &signed_tx).unwrap();
                    tx_num += 1;
                }
                txs_writer.increment_block(block_number).unwrap();
            }
            txs_writer.commit().unwrap();
        }

        let mut tx_num = 0u64;
        db.commit(|tx| {
            for &(block_number, ref txs) in blocks {
                let first_tx_num = tx_num;
                let tx_count = txs.len() as u64;

                tx.put::<tables::BlockBodyIndices>(
                    block_number,
                    StoredBlockBodyIndices { first_tx_num, tx_count },
                )?;

                for (sender, _to_kind) in txs {
                    tx.put::<tables::TransactionSenders>(tx_num, *sender)?;
                    tx_num += 1;
                }
            }
            Ok(())
        })
        .unwrap();
    }

    fn run(db: &TestStageDB, run_to: u64) {
        let input = ExecInput { target: Some(run_to), ..Default::default() };
        let mut stage = IndexTransactionHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(run_to), done: true });
        provider.commit().unwrap();
    }

    fn unwind(db: &TestStageDB, unwind_from: u64, unwind_to: u64) {
        let input = UnwindInput {
            checkpoint: StageCheckpoint::new(unwind_from),
            unwind_to,
            ..Default::default()
        };
        let mut stage = IndexTransactionHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.unwind(&provider, input).unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
        provider.commit().unwrap();
    }

    #[tokio::test]
    async fn test_basic_execute() {
        let db = TestStageDB::default();

        // Block 0: sender_a → recipient_x
        // Block 1: sender_b → recipient_x
        // Block 2: sender_a → recipient_y
        setup_test_data(
            &db,
            &[
                (0, vec![(SENDER_A, TxKind::Call(RECIPIENT_X))]),
                (1, vec![(SENDER_B, TxKind::Call(RECIPIENT_X))]),
                (2, vec![(SENDER_A, TxKind::Call(RECIPIENT_Y))]),
            ],
        );

        run(&db, 2);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        assert_eq!(
            from_table,
            BTreeMap::from([
                (shard(SENDER_A, u64::MAX), vec![0, 2]),
                (shard(SENDER_B, u64::MAX), vec![1]),
            ])
        );

        let to_table = cast(db.table::<tables::CallTraceToIndex>().unwrap());
        assert_eq!(
            to_table,
            BTreeMap::from([
                (shard(RECIPIENT_X, u64::MAX), vec![0, 1]),
                (shard(RECIPIENT_Y, u64::MAX), vec![2]),
            ])
        );
    }

    #[tokio::test]
    async fn test_unwind() {
        let db = TestStageDB::default();

        setup_test_data(
            &db,
            &[
                (0, vec![(SENDER_A, TxKind::Call(RECIPIENT_X))]),
                (1, vec![(SENDER_B, TxKind::Call(RECIPIENT_X))]),
                (2, vec![(SENDER_A, TxKind::Call(RECIPIENT_Y))]),
            ],
        );

        run(&db, 2);
        unwind(&db, 2, 0);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        assert_eq!(from_table, BTreeMap::from([(shard(SENDER_A, u64::MAX), vec![0])]));

        let to_table = cast(db.table::<tables::CallTraceToIndex>().unwrap());
        assert_eq!(to_table, BTreeMap::from([(shard(RECIPIENT_X, u64::MAX), vec![0])]));
    }

    #[tokio::test]
    async fn test_shard_boundary() {
        let db = TestStageDB::default();

        let total_blocks = NUM_OF_INDICES_IN_SHARD as u64 + 2;
        let blocks: Vec<(u64, Vec<(Address, TxKind)>)> =
            (0..total_blocks).map(|b| (b, vec![(SENDER_A, TxKind::Call(RECIPIENT_X))])).collect();

        setup_test_data(&db, &blocks);

        run(&db, total_blocks - 1);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        let full_shard: Vec<u64> = (0..NUM_OF_INDICES_IN_SHARD as u64).collect();
        let last_block = NUM_OF_INDICES_IN_SHARD as u64 - 1;
        let remaining: Vec<u64> = (NUM_OF_INDICES_IN_SHARD as u64..total_blocks).collect();

        assert_eq!(
            from_table,
            BTreeMap::from([
                (shard(SENDER_A, last_block), full_shard),
                (shard(SENDER_A, u64::MAX), remaining),
            ])
        );
    }

    #[tokio::test]
    async fn test_contract_creation_no_to_index() {
        let db = TestStageDB::default();

        // Block 0: sender_a creates a contract (TxKind::Create)
        // Block 1: empty (target must be > 0 for the stage to execute on first sync)
        setup_test_data(&db, &[(0, vec![(SENDER_A, TxKind::Create)]), (1, vec![])]);

        run(&db, 1);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        assert_eq!(from_table, BTreeMap::from([(shard(SENDER_A, u64::MAX), vec![0])]));

        let to_table = cast(db.table::<tables::CallTraceToIndex>().unwrap());
        assert!(to_table.is_empty(), "Create transactions should not produce to-index entries");
    }

    #[tokio::test]
    async fn test_empty_blocks() {
        let db = TestStageDB::default();

        // Three blocks with zero transactions
        setup_test_data(&db, &[(0, vec![]), (1, vec![]), (2, vec![])]);

        run(&db, 2);

        let from_table = db.table::<tables::CallTraceFromIndex>().unwrap();
        assert!(from_table.is_empty());

        let to_table = db.table::<tables::CallTraceToIndex>().unwrap();
        assert!(to_table.is_empty());
    }

    #[tokio::test]
    async fn test_unwind_across_shard_boundary() {
        let db = TestStageDB::default();

        let total_blocks = NUM_OF_INDICES_IN_SHARD as u64 + 2;
        let blocks: Vec<(u64, Vec<(Address, TxKind)>)> =
            (0..total_blocks).map(|b| (b, vec![(SENDER_A, TxKind::Call(RECIPIENT_X))])).collect();

        setup_test_data(&db, &blocks);
        run(&db, total_blocks - 1);

        // Verify two shards exist before unwind
        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        assert_eq!(from_table.len(), 2);

        // Unwind into the first shard (keep half of the first shard's blocks)
        let unwind_to = NUM_OF_INDICES_IN_SHARD as u64 / 2;
        unwind(&db, total_blocks - 1, unwind_to);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        let expected: Vec<u64> = (0..=unwind_to).collect();
        assert_eq!(from_table, BTreeMap::from([(shard(SENDER_A, u64::MAX), expected)]));

        let to_table = cast(db.table::<tables::CallTraceToIndex>().unwrap());
        let expected: Vec<u64> = (0..=unwind_to).collect();
        assert_eq!(to_table, BTreeMap::from([(shard(RECIPIENT_X, u64::MAX), expected)]));
    }

    #[tokio::test]
    async fn test_partial_unwind() {
        let db = TestStageDB::default();

        // Block 0: sender_a → recipient_x
        // Block 1: sender_b → recipient_x
        // Block 2: sender_a → recipient_y
        setup_test_data(
            &db,
            &[
                (0, vec![(SENDER_A, TxKind::Call(RECIPIENT_X))]),
                (1, vec![(SENDER_B, TxKind::Call(RECIPIENT_X))]),
                (2, vec![(SENDER_A, TxKind::Call(RECIPIENT_Y))]),
            ],
        );

        run(&db, 2);
        // Unwind to block 1 — block 2 entries should be removed
        unwind(&db, 2, 1);

        let from_table = cast(db.table::<tables::CallTraceFromIndex>().unwrap());
        assert_eq!(
            from_table,
            BTreeMap::from([
                (shard(SENDER_A, u64::MAX), vec![0]),
                (shard(SENDER_B, u64::MAX), vec![1]),
            ])
        );

        let to_table = cast(db.table::<tables::CallTraceToIndex>().unwrap());
        assert_eq!(to_table, BTreeMap::from([(shard(RECIPIENT_X, u64::MAX), vec![0, 1])]));
    }
}
