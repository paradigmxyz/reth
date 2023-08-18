//! Support for pruning.

use crate::{Metrics, PrunerError};
use rayon::prelude::*;
use reth_db::{
    abstraction::cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress, ShardedKey},
    table::Table,
    tables,
    transaction::DbTxMut,
    BlockNumberList,
};
use reth_primitives::{
    BlockNumber, ChainSpec, PruneCheckpoint, PruneMode, PruneModes, PrunePart, TxNumber,
};
use reth_provider::{
    BlockReader, DatabaseProviderRW, ProviderFactory, PruneCheckpointReader, PruneCheckpointWriter,
    TransactionsProvider,
};
use std::{ops::RangeInclusive, sync::Arc, time::Instant};
use tracing::{debug, error, instrument, trace};

/// Result of [Pruner::run] execution.
///
/// Returns `true` if pruning has been completed up to the target block,
/// and `false` if there's more data to prune in further runs.
pub type PrunerResult = Result<bool, PrunerError>;

/// The pruner type itself with the result of [Pruner::run]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

pub struct BatchSizes {
    /// Maximum number of receipts to prune in one run.
    receipts: usize,
    /// Maximum number of transaction lookup entries to prune in one run.
    transaction_lookup: usize,
    /// Maximum number of transaction senders to prune in one run.
    transaction_senders: usize,
    /// Maximum number of account history entries to prune in one run.
    /// Measured in the number of [tables::AccountChangeSet] rows.
    account_history: usize,
    /// Maximum number of storage history entries to prune in one run.
    /// Measured in the number of [tables::StorageChangeSet] rows.
    storage_history: usize,
}

impl Default for BatchSizes {
    fn default() -> Self {
        Self {
            receipts: 1000,
            transaction_lookup: 1000,
            transaction_senders: 1000,
            account_history: 1000,
            storage_history: 1000,
        }
    }
}

/// Pruning routine. Main pruning logic happens in [Pruner::run].
pub struct Pruner<DB> {
    metrics: Metrics,
    provider_factory: ProviderFactory<DB>,
    /// Minimum pruning interval measured in blocks. All prune parts are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: u64,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
    modes: PruneModes,
    /// Maximum entries to prune per one run, per prune part.
    batch_sizes: BatchSizes,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        min_block_interval: u64,
        modes: PruneModes,
        batch_sizes: BatchSizes,
    ) -> Self {
        Self {
            metrics: Metrics::default(),
            provider_factory: ProviderFactory::new(db, chain_spec),
            min_block_interval,
            last_pruned_block_number: None,
            modes,
            batch_sizes,
        }
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        trace!(
            target: "pruner",
            %tip_block_number,
            "Pruner started"
        );
        let start = Instant::now();

        let provider = self.provider_factory.provider_rw()?;

        let mut done = true;

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_receipts(tip_block_number)?
        {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::Receipts,
                %to_block,
                ?prune_mode,
                "Got target block to prune"
            );

            let part_start = Instant::now();
            let part_done = self.prune_receipts(&provider, to_block, prune_mode)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::Receipts)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::Receipts,
                "No target block to prune"
            );
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_transaction_lookup(tip_block_number)?
        {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::TransactionLookup,
                %to_block,
                ?prune_mode,
                "Got target block to prune"
            );

            let part_start = Instant::now();
            let part_done = self.prune_transaction_lookup(&provider, to_block, prune_mode)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::TransactionLookup)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::TransactionLookup,
                "No target block to prune"
            );
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_sender_recovery(tip_block_number)?
        {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::SenderRecovery,
                %to_block,
                ?prune_mode,
                "Got target block to prune"
            );

            let part_start = Instant::now();
            let part_done = self.prune_transaction_senders(&provider, to_block, prune_mode)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::SenderRecovery)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::SenderRecovery,
                "No target block to prune"
            );
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_account_history(tip_block_number)?
        {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::AccountHistory,
                %to_block,
                ?prune_mode,
                "Got target block to prune"
            );

            let part_start = Instant::now();
            let part_done = self.prune_account_history(&provider, to_block, prune_mode)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::AccountHistory)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::AccountHistory,
                "No target block to prune"
            );
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_storage_history(tip_block_number)?
        {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::StorageHistory,
                %to_block,
                ?prune_mode,
                "Got target block to prune"
            );

            let part_start = Instant::now();
            let part_done = self.prune_storage_history(&provider, to_block, prune_mode)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::StorageHistory)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(
                target: "pruner",
                prune_part = ?PrunePart::StorageHistory,
                "No target block to prune"
            );
        }

        provider.commit()?;
        self.last_pruned_block_number = Some(tip_block_number);

        let elapsed = start.elapsed();
        self.metrics.duration_seconds.record(elapsed);

        trace!(
            target: "pruner",
            %tip_block_number,
            ?elapsed,
            "Pruner finished"
        );
        Ok(done)
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        if self.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previously pruned block number. If
            // that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(last_pruned_block_number) >= self.min_block_interval
        }) {
            debug!(
                target: "pruner",
                last_pruned_block_number = ?self.last_pruned_block_number,
                %tip_block_number,
                "Minimum pruning interval reached"
            );
            true
        } else {
            false
        }
    }

    /// Get next inclusive block range to prune according to the checkpoint, `to_block` block
    /// number and `limit`.
    ///
    /// To get the range start (`from_block`):
    /// 1. If checkpoint exists, use next block.
    /// 2. If checkpoint doesn't exist, use block 0.
    ///
    /// To get the range end: use block `to_block`.
    fn get_next_block_range_from_checkpoint(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        prune_part: PrunePart,
        to_block: BlockNumber,
    ) -> reth_interfaces::Result<Option<RangeInclusive<BlockNumber>>> {
        let from_block = provider
            .get_prune_checkpoint(prune_part)?
            .and_then(|checkpoint| checkpoint.block_number)
            // Checkpoint exists, prune from the next block after the highest pruned one
            .map(|block_number| block_number + 1)
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let range = from_block..=to_block;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
    }

    /// Get next inclusive tx number range to prune according to the checkpoint and `to_block` block
    /// number.
    ///
    /// To get the range start:
    /// 1. If checkpoint exists, get next block body and return its first tx number.
    /// 2. If checkpoint doesn't exist, return 0.
    ///
    /// To get the range end: get last tx number for the provided `to_block`.
    fn get_next_tx_num_range_from_checkpoint(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        prune_part: PrunePart,
        to_block: BlockNumber,
    ) -> reth_interfaces::Result<Option<RangeInclusive<TxNumber>>> {
        let from_tx_number = provider
            .get_prune_checkpoint(prune_part)?
            // Checkpoint exists, prune from the next transaction after the highest pruned one
            .and_then(|checkpoint| match checkpoint.tx_number {
                Some(tx_number) => Some(tx_number + 1),
                _ => {
                    error!(target: "pruner", %prune_part, ?checkpoint, "Expected transaction number in prune checkpoint, found None");
                    None
                },
            })
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let to_tx_number = match provider.block_body_indices(to_block)? {
            Some(body) => body,
            None => return Ok(None),
        }
        .last_tx_num();

        Ok(Some(from_tx_number..=to_tx_number))
    }

    /// Prune receipts up to the provided block, inclusive, respecting the batch size.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_receipts(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let tx_range = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::Receipts,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No receipts to prune");
                return Ok(true)
            }
        };
        let tx_range_end = *tx_range.end();

        let mut last_pruned_transaction = tx_range_end;
        let (deleted, done) = provider.prune_table_with_range::<tables::Receipts>(
            tx_range,
            self.batch_sizes.receipts,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %deleted, %done, "Pruned receipts");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more receipts to prune, set the checkpoint block number to previous,
            // so we could finish pruning its receipts on the next run.
            .checked_sub(if done { 0 } else { 1 });

        provider.save_prune_checkpoint(
            PrunePart::Receipts,
            PruneCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
                prune_mode,
            },
        )?;

        Ok(done)
    }

    /// Prune transaction lookup entries up to the provided block, inclusive, respecting the batch
    /// size.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_transaction_lookup(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let (start, end) = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::TransactionLookup,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction lookup entries to prune");
                return Ok(true)
            }
        }
        .into_inner();
        let tx_range = start..=(end.min(start + self.batch_sizes.transaction_lookup as u64 - 1));
        let tx_range_end = *tx_range.end();

        // Retrieve transactions in the range and calculate their hashes in parallel
        let hashes = provider
            .transactions_by_tx_range(tx_range.clone())?
            .into_par_iter()
            .map(|transaction| transaction.hash())
            .collect::<Vec<_>>();

        // Number of transactions retrieved from the database should match the tx range count
        let tx_count = tx_range.clone().count();
        if hashes.len() != tx_count {
            return Err(PrunerError::InconsistentData(
                "Unexpected number of transaction hashes retrieved by transaction number range",
            ))
        }

        let mut last_pruned_transaction = tx_range_end;
        let (deleted, done) = provider.prune_table_with_iterator::<tables::TxHashNumber>(
            hashes,
            self.batch_sizes.transaction_lookup,
            |row| last_pruned_transaction = row.1,
        )?;
        trace!(target: "pruner", %deleted, %done, "Pruned transaction lookup");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction lookup entries to prune, set the checkpoint block number
            // to previous, so we could finish pruning its transaction lookup entries on the next
            // run.
            .checked_sub(if done { 0 } else { 1 });

        provider.save_prune_checkpoint(
            PrunePart::TransactionLookup,
            PruneCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
                prune_mode,
            },
        )?;

        Ok(done)
    }

    /// Prune transaction senders up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_transaction_senders(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let tx_range = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::SenderRecovery,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction senders to prune");
                return Ok(true)
            }
        };
        let tx_range_end = *tx_range.end();

        let mut last_pruned_transaction = tx_range_end;
        let (deleted, done) = provider.prune_table_with_range::<tables::TxSenders>(
            tx_range,
            self.batch_sizes.transaction_senders,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %deleted, %done, "Pruned transaction senders");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction senders to prune, set the checkpoint block number to
            // previous, so we could finish pruning its transaction senders on the next run.
            .checked_sub(if done { 0 } else { 1 });

        provider.save_prune_checkpoint(
            PrunePart::SenderRecovery,
            PruneCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
                prune_mode,
            },
        )?;

        Ok(done)
    }

    /// Prune account history up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_account_history(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let range = match self.get_next_block_range_from_checkpoint(
            provider,
            PrunePart::AccountHistory,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account history to prune");
                return Ok(true)
            }
        };
        let range_end = *range.end();

        let mut last_pruned_block_number = None;
        let (rows, done) = provider.prune_table_with_range::<tables::AccountChangeSet>(
            range,
            self.batch_sizes.account_history,
            |row| last_pruned_block_number = Some(row.0),
        )?;
        trace!(target: "pruner", %rows, %done, "Pruned account history (changesets)");

        let last_pruned_block = last_pruned_block_number
            // If there's more account account changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its account changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let (processed, deleted) = self.prune_history_indices::<tables::AccountHistory, _>(
            provider,
            last_pruned_block,
            |a, b| a.key == b.key,
            |key| ShardedKey::last(key.key),
        )?;
        trace!(target: "pruner", %processed, %deleted, %done, "Pruned account history (history)" );

        provider.save_prune_checkpoint(
            PrunePart::AccountHistory,
            PruneCheckpoint { block_number: Some(last_pruned_block), tx_number: None, prune_mode },
        )?;

        Ok(done)
    }

    /// Prune storage history up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_storage_history(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let range = match self.get_next_block_range_from_checkpoint(
            provider,
            PrunePart::StorageHistory,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No storage history to prune");
                return Ok(true)
            }
        };
        let range_end = *range.end();

        let mut last_pruned_block_number = None;
        let (rows, done) = provider.prune_table_with_range::<tables::StorageChangeSet>(
            BlockNumberAddress::range(range),
            self.batch_sizes.storage_history,
            |row| last_pruned_block_number = Some(row.0.block_number()),
        )?;
        trace!(target: "pruner", %rows, %done, "Pruned storage history (changesets)");

        let last_pruned_block = last_pruned_block_number
            // If there's more account storage changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its storage changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let (processed, deleted) = self.prune_history_indices::<tables::StorageHistory, _>(
            provider,
            last_pruned_block,
            |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
            |key| StorageShardedKey::last(key.address, key.sharded_key.key),
        )?;
        trace!(target: "pruner", %processed, %deleted, %done, "Pruned storage history (history)" );

        provider.save_prune_checkpoint(
            PrunePart::StorageHistory,
            PruneCheckpoint { block_number: Some(last_pruned_block), tx_number: None, prune_mode },
        )?;

        Ok(done)
    }

    /// Prune history indices up to the provided block, inclusive.
    fn prune_history_indices<T, SK>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        key_matches: impl Fn(&T::Key, &T::Key) -> bool,
        last_key: impl Fn(&T::Key) -> T::Key,
    ) -> Result<(usize, usize), PrunerError>
    where
        T: Table<Value = BlockNumberList>,
        T::Key: AsRef<ShardedKey<SK>>,
    {
        let mut processed = 0;
        let mut deleted = 0;
        let mut cursor = provider.tx_ref().cursor_write::<T>()?;

        // Prune history table:
        // 1. If the shard has `highest_block_number` less than or equal to the target block number
        // for pruning, delete the shard completely.
        // 2. If the shard has `highest_block_number` greater than the target block number for
        // pruning, filter block numbers inside the shard which are less than the target
        // block number for pruning.
        while let Some(result) = cursor.next()? {
            let (key, blocks): (T::Key, BlockNumberList) = result;

            if key.as_ref().highest_block_number <= to_block {
                // If shard consists only of block numbers less than the target one, delete shard
                // completely.
                cursor.delete_current()?;
                deleted += 1;
                if key.as_ref().highest_block_number == to_block {
                    // Shard contains only block numbers up to the target one, so we can skip to the
                    // next sharded key. It is guaranteed that further shards for this sharded key
                    // will not contain the target block number, as it's in this shard.
                    cursor.seek_exact(last_key(&key))?;
                }
            } else {
                // Shard contains block numbers that are higher than the target one, so we need to
                // filter it. It is guaranteed that further shards for this sharded key will not
                // contain the target block number, as it's in this shard.
                let new_blocks = blocks
                    .iter(0)
                    .skip_while(|block| *block <= to_block as usize)
                    .collect::<Vec<_>>();

                if blocks.len() != new_blocks.len() {
                    // If there were blocks less than or equal to the target one
                    // (so the shard has changed), update the shard.
                    if new_blocks.is_empty() {
                        // If there are no more blocks in this shard, we need to remove it, as empty
                        // shards are not allowed.
                        if key.as_ref().highest_block_number == u64::MAX {
                            if let Some(prev_value) = cursor
                                .prev()?
                                .filter(|(prev_key, _)| key_matches(prev_key, &key))
                                .map(|(_, prev_value)| prev_value)
                            {
                                // If current shard is the last shard for the sharded key that has
                                // previous shards, replace it with the previous shard.
                                cursor.delete_current()?;
                                deleted += 1;
                                // Upsert will replace the last shard for this sharded key with the
                                // previous value.
                                cursor.upsert(key.clone(), prev_value)?;
                            } else {
                                // If there's no previous shard for this sharded key,
                                // just delete last shard completely.

                                // Jump back to the original last shard.
                                cursor.next()?;
                                // Delete shard.
                                cursor.delete_current()?;
                                deleted += 1;
                            }
                        } else {
                            // If current shard is not the last shard for this sharded key,
                            // just delete it.
                            cursor.delete_current()?;
                            deleted += 1;
                        }
                    } else {
                        cursor.upsert(key.clone(), BlockNumberList::new_pre_sorted(new_blocks))?;
                    }
                }

                // Jump to the next address.
                cursor.seek_exact(last_key(&key))?;
            }

            processed += 1;
        }

        Ok((processed, deleted))
    }
}

#[cfg(test)]
mod tests {
    use crate::{pruner::BatchSizes, Pruner};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::{tables, test_utils::create_test_rw_db, BlockNumberList};
    use reth_interfaces::test_utils::{
        generators,
        generators::{
            random_block_range, random_changeset_range, random_eoa_account_range, random_receipt,
        },
    };
    use reth_primitives::{
        BlockNumber, PruneCheckpoint, PruneMode, PruneModes, PrunePart, TxNumber, H256, MAINNET,
    };
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestTransaction;
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let pruner =
            Pruner::new(db, MAINNET.clone(), 5, PruneModes::default(), BatchSizes::default());

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));

        // Delta is not less than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval;
        assert!(pruner.is_pruning_needed(second_block_number));

        // Delta is less than min block interval
        let third_block_number = second_block_number;
        assert!(pruner.is_pruning_needed(third_block_number));
    }

    #[test]
    fn prune_receipts() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=100, H256::zero(), 0..10);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                receipts
                    .push((receipts.len() as u64, random_receipt(&mut rng, transaction, Some(0))));
            }
        }
        tx.insert_receipts(receipts).expect("insert receipts");

        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            tx.table::<tables::Receipts>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { receipts: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    receipts: 10,
                    ..Default::default()
                },
            );

            let next_tx_number_to_prune = tx
                .inner()
                .get_prune_checkpoint(PrunePart::Receipts)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number = blocks
                .iter()
                .map(|block| block.body.len())
                .sum::<usize>()
                .min(next_tx_number_to_prune as usize + pruner.batch_sizes.receipts - 1);

            let last_pruned_block_number = blocks
                .iter()
                .fold_while((0, 0), |(_, mut tx_count), block| {
                    tx_count += block.body.len();

                    if tx_count > last_pruned_tx_number {
                        Done((block.number, tx_count))
                    } else {
                        Continue((block.number, tx_count))
                    }
                })
                .into_inner()
                .0;

            let provider = tx.inner_rw();
            let result = pruner.prune_receipts(&provider, to_block, prune_mode);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number =
                last_pruned_block_number.checked_sub(if done { 0 } else { 1 });

            assert_eq!(
                tx.table::<tables::Receipts>().unwrap().len(),
                blocks.iter().map(|block| block.body.len()).sum::<usize>() -
                    (last_pruned_tx_number + 1)
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::Receipts).unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(15);
        test_prune(15);
        test_prune(20);
    }

    #[test]
    fn prune_transaction_lookup() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=100, H256::zero(), 0..10);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                tx_hash_numbers.push((transaction.hash, tx_hash_numbers.len() as u64));
            }
        }
        tx.insert_tx_hash_numbers(tx_hash_numbers).expect("insert tx hash numbers");

        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            tx.table::<tables::TxHashNumber>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { transaction_lookup: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    transaction_lookup: 10,
                    ..Default::default()
                },
            );

            let next_tx_number_to_prune = tx
                .inner()
                .get_prune_checkpoint(PrunePart::TransactionLookup)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number =
                blocks.iter().map(|block| block.body.len()).sum::<usize>().min(
                    next_tx_number_to_prune as usize + pruner.batch_sizes.transaction_lookup - 1,
                );

            let last_pruned_block_number = blocks
                .iter()
                .fold_while((0, 0), |(_, mut tx_count), block| {
                    tx_count += block.body.len();

                    if tx_count > last_pruned_tx_number {
                        Done((block.number, tx_count))
                    } else {
                        Continue((block.number, tx_count))
                    }
                })
                .into_inner()
                .0;

            let provider = tx.inner_rw();
            let result = pruner.prune_transaction_lookup(&provider, to_block, prune_mode);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number =
                last_pruned_block_number.checked_sub(if done { 0 } else { 1 });

            assert_eq!(
                tx.table::<tables::TxHashNumber>().unwrap().len(),
                blocks.iter().map(|block| block.body.len()).sum::<usize>() -
                    (last_pruned_tx_number + 1)
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::TransactionLookup).unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(15);
        test_prune(15);
        test_prune(20);
    }

    #[test]
    fn prune_transaction_senders() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=100, H256::zero(), 0..10);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut transaction_senders = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                transaction_senders.push((
                    transaction_senders.len() as u64,
                    transaction.recover_signer().expect("recover signer"),
                ));
            }
        }
        tx.insert_transaction_senders(transaction_senders).expect("insert transaction senders");

        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            tx.table::<tables::Transactions>().unwrap().len(),
            tx.table::<tables::TxSenders>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { sender_recovery: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    transaction_senders: 10,
                    ..Default::default()
                },
            );

            let next_tx_number_to_prune = tx
                .inner()
                .get_prune_checkpoint(PrunePart::SenderRecovery)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number =
                blocks.iter().map(|block| block.body.len()).sum::<usize>().min(
                    next_tx_number_to_prune as usize + pruner.batch_sizes.transaction_senders - 1,
                );

            let last_pruned_block_number = blocks
                .iter()
                .fold_while((0, 0), |(_, mut tx_count), block| {
                    tx_count += block.body.len();

                    if tx_count > last_pruned_tx_number {
                        Done((block.number, tx_count))
                    } else {
                        Continue((block.number, tx_count))
                    }
                })
                .into_inner()
                .0;

            let provider = tx.inner_rw();
            let result = pruner.prune_transaction_senders(&provider, to_block, prune_mode);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number =
                last_pruned_block_number.checked_sub(if done { 0 } else { 1 });

            assert_eq!(
                tx.table::<tables::TxSenders>().unwrap().len(),
                blocks.iter().map(|block| block.body.len()).sum::<usize>() -
                    (last_pruned_tx_number + 1)
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::SenderRecovery).unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(15);
        test_prune(15);
        test_prune(20);
    }

    #[test]
    fn prune_account_history() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=7000, H256::zero(), 0..1);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let accounts =
            random_eoa_account_range(&mut rng, 0..3).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            0..0,
            0..0,
        );
        tx.insert_changesets(changesets.clone(), None).expect("insert changesets");
        tx.insert_history(changesets.clone(), None).expect("insert history");

        let account_occurrences = tx.table::<tables::AccountHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry(key.key).or_default().add_assign(1);
                map
            },
        );
        assert!(account_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            tx.table::<tables::AccountChangeSet>().unwrap().len(),
            changesets.iter().flatten().count()
        );

        let original_shards = tx.table::<tables::AccountHistory>().unwrap();

        let test_prune = |to_block: BlockNumber, run: usize, expect_done: bool| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { account_history: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    account_history: 2000,
                    ..Default::default()
                },
            );

            let provider = tx.inner_rw();
            let result = pruner.prune_account_history(&provider, to_block, prune_mode);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            assert_eq!(done, expect_done);
            provider.commit().expect("commit");

            let changesets = changesets
                .iter()
                .enumerate()
                .flat_map(|(block_number, changeset)| {
                    changeset.into_iter().map(move |change| (block_number, change))
                })
                .collect::<Vec<_>>();

            let pruned = changesets
                .iter()
                .enumerate()
                .skip_while(|(i, (block_number, _))| {
                    *i < pruner.batch_sizes.account_history * run &&
                        *block_number <= to_block as usize
                })
                .next()
                .map(|(i, _)| i)
                .unwrap_or_default();

            let mut pruned_changesets = changesets
                .iter()
                // Skip what we've pruned so far, subtracting one to get last pruned block number
                // further down
                .skip(pruned.saturating_sub(1));

            let last_pruned_block_number = pruned_changesets
                .next()
                .map(|(block_number, _)| if done { *block_number } else { block_number.saturating_sub(1) } as BlockNumber)
                .unwrap_or(to_block);

            let pruned_changesets =
                pruned_changesets.fold(BTreeMap::new(), |mut acc, (block_number, change)| {
                    acc.entry(block_number).or_insert_with(Vec::new).push(change);
                    acc
                });

            assert_eq!(
                tx.table::<tables::AccountChangeSet>().unwrap().len(),
                pruned_changesets.values().flatten().count()
            );

            let actual_shards = tx.table::<tables::AccountHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.highest_block_number > last_pruned_block_number)
                .map(|(key, blocks)| {
                    let new_blocks = blocks
                        .iter(0)
                        .skip_while(|block| *block <= last_pruned_block_number as usize)
                        .collect::<Vec<_>>();
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::AccountHistory).unwrap(),
                Some(PruneCheckpoint {
                    block_number: Some(last_pruned_block_number),
                    tx_number: None,
                    prune_mode
                })
            );
        };

        test_prune(1700, 1, false);
        test_prune(1700, 2, true);
        test_prune(2000, 3, true);
    }

    #[test]
    fn prune_storage_history() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=7000, H256::zero(), 0..1);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let accounts =
            random_eoa_account_range(&mut rng, 0..3).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            1..2,
            1..2,
        );
        tx.insert_changesets(changesets.clone(), None).expect("insert changesets");
        tx.insert_history(changesets.clone(), None).expect("insert history");

        let storage_occurences = tx.table::<tables::StorageHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry((key.address, key.sharded_key.key)).or_default().add_assign(1);
                map
            },
        );
        assert!(storage_occurences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            tx.table::<tables::StorageChangeSet>().unwrap().len(),
            changesets.iter().flatten().flat_map(|(_, _, entries)| entries).count()
        );

        let original_shards = tx.table::<tables::StorageHistory>().unwrap();

        let test_prune = |to_block: BlockNumber, run: usize, expect_done: bool| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { storage_history: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    storage_history: 2000,
                    ..Default::default()
                },
            );

            let provider = tx.inner_rw();
            let result = pruner.prune_storage_history(&provider, to_block, prune_mode);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            assert_eq!(done, expect_done);
            provider.commit().expect("commit");

            let changesets = changesets
                .iter()
                .enumerate()
                .flat_map(|(block_number, changeset)| {
                    changeset.into_iter().flat_map(move |(address, _, entries)| {
                        entries.into_iter().map(move |entry| (block_number, address, entry))
                    })
                })
                .collect::<Vec<_>>();

            let pruned = changesets
                .iter()
                .enumerate()
                .skip_while(|(i, (block_number, _, _))| {
                    *i < pruner.batch_sizes.storage_history * run &&
                        *block_number <= to_block as usize
                })
                .next()
                .map(|(i, _)| i)
                .unwrap_or_default();

            let mut pruned_changesets = changesets
                .iter()
                // Skip what we've pruned so far, subtracting one to get last pruned block number
                // further down
                .skip(pruned.saturating_sub(1));

            let last_pruned_block_number = pruned_changesets
                .next()
                .map(|(block_number, _, _)| if done { *block_number } else { block_number.saturating_sub(1) } as BlockNumber)
                .unwrap_or(to_block);

            let pruned_changesets = pruned_changesets.fold(
                BTreeMap::new(),
                |mut acc, (block_number, address, entry)| {
                    acc.entry((block_number, address)).or_insert_with(Vec::new).push(entry);
                    acc
                },
            );

            assert_eq!(
                tx.table::<tables::StorageChangeSet>().unwrap().len(),
                pruned_changesets.values().flatten().count()
            );

            let actual_shards = tx.table::<tables::StorageHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.sharded_key.highest_block_number > last_pruned_block_number)
                .map(|(key, blocks)| {
                    let new_blocks = blocks
                        .iter(0)
                        .skip_while(|block| *block <= last_pruned_block_number as usize)
                        .collect::<Vec<_>>();
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::StorageHistory).unwrap(),
                Some(PruneCheckpoint {
                    block_number: Some(last_pruned_block_number),
                    tx_number: None,
                    prune_mode
                })
            );
        };

        test_prune(2300, 1, false);
        test_prune(2300, 2, true);
        test_prune(3000, 3, true);
    }
}
