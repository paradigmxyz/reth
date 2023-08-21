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
    MINIMUM_PRUNING_DISTANCE,
};
use reth_provider::{
    BlockReader, DatabaseProviderRW, ProviderFactory, PruneCheckpointReader, PruneCheckpointWriter,
    TransactionsProvider,
};
use std::{ops::RangeInclusive, sync::Arc, time::Instant};
use tracing::{debug, instrument, trace};

/// Result of [Pruner::run] execution
pub type PrunerResult = Result<(), PrunerError>;

/// The pipeline type itself with the result of [Pruner::run]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

pub struct BatchSizes {
    receipts: usize,
    transaction_lookup: usize,
    transaction_senders: usize,
    account_history: usize,
    storage_history: usize,
}

impl Default for BatchSizes {
    fn default() -> Self {
        Self {
            receipts: 10000,
            transaction_lookup: 10000,
            transaction_senders: 10000,
            account_history: 10000,
            storage_history: 10000,
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

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_receipts(tip_block_number)?
        {
            let part_start = Instant::now();
            self.prune_receipts(&provider, to_block, prune_mode)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::Receipts)
                .duration_seconds
                .record(part_start.elapsed())
        }

        if !self.modes.contract_logs_filter.is_empty() {
            let part_start = Instant::now();
            self.prune_receipts_by_logs(&provider, tip_block_number)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::ContractLogs)
                .duration_seconds
                .record(part_start.elapsed())
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_transaction_lookup(tip_block_number)?
        {
            let part_start = Instant::now();
            self.prune_transaction_lookup(&provider, to_block, prune_mode)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::TransactionLookup)
                .duration_seconds
                .record(part_start.elapsed())
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_sender_recovery(tip_block_number)?
        {
            let part_start = Instant::now();
            self.prune_transaction_senders(&provider, to_block, prune_mode)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::SenderRecovery)
                .duration_seconds
                .record(part_start.elapsed())
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_account_history(tip_block_number)?
        {
            let part_start = Instant::now();
            self.prune_account_history(&provider, to_block, prune_mode)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::AccountHistory)
                .duration_seconds
                .record(part_start.elapsed())
        }

        if let Some((to_block, prune_mode)) =
            self.modes.prune_target_block_storage_history(tip_block_number)?
        {
            let part_start = Instant::now();
            self.prune_storage_history(&provider, to_block, prune_mode)?;
            self.metrics
                .get_prune_part_metrics(PrunePart::StorageHistory)
                .duration_seconds
                .record(part_start.elapsed())
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
        Ok(())
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
        let from_block_number = provider
            .get_prune_checkpoint(prune_part)?
            // Checkpoint exists, prune from the next block after the highest pruned one
            .map(|checkpoint| checkpoint.block_number + 1)
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        // Get first transaction
        let from_tx_num =
            provider.block_body_indices(from_block_number)?.map(|body| body.first_tx_num);
        // If no block body index is found, the DB is either corrupted or we've already pruned up to
        // the latest block, so there's no thing to prune now.
        let Some(from_tx_num) = from_tx_num else { return Ok(None) };

        let to_tx_num = match provider.block_body_indices(to_block)? {
            Some(body) => body,
            None => return Ok(None),
        }
        .last_tx_num();

        Ok(Some(from_tx_num..=to_tx_num))
    }

    /// Prune receipts up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_receipts(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let range = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::Receipts,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No receipts to prune");
                return Ok(())
            }
        };
        let total = range.clone().count();

        provider.prune_table_with_iterator_in_batches::<tables::Receipts>(
            range,
            self.batch_sizes.receipts,
            |rows| {
                trace!(
                    target: "pruner",
                    %rows,
                    progress = format!("{:.1}%", 100.0 * rows as f64 / total as f64),
                    "Pruned receipts"
                );
            },
            |_| false,
        )?;

        provider.save_prune_checkpoint(
            PrunePart::Receipts,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        // `PrunePart::Receipts` overrides `PrunePart::ContractLogs`, so we can preemptively
        // limit their pruning start point.
        provider.save_prune_checkpoint(
            PrunePart::ContractLogs,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }

    /// Prune receipts up to the provided block by filtering logs. Works as in inclusion list, and
    /// removes every receipt not belonging to it.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_receipts_by_logs(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        tip_block_number: BlockNumber,
    ) -> PrunerResult {
        // Contract log filtering removes every receipt possible except the ones in the list. So,
        // for the other receipts it's as if they had a `PruneMode::Distance()` of 128.
        let to_block = PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)
            .prune_target_block(
                tip_block_number,
                MINIMUM_PRUNING_DISTANCE,
                PrunePart::ContractLogs,
            )?
            .map(|(bn, _)| bn)
            .unwrap_or_default();

        // Figure out what receipts have already been pruned, so we can have an accurate
        // `address_filter`
        let pruned = provider
            .get_prune_checkpoint(PrunePart::ContractLogs)?
            .map(|checkpoint| checkpoint.block_number);

        let address_filter =
            self.modes.contract_logs_filter.group_by_block(tip_block_number, pruned)?;

        // Splits all transactions in different block ranges. Each block range will have its own
        // filter address list and will check it while going through the table
        //
        // Example:
        // For an `address_filter` such as:
        // { block9: [a1, a2], block20: [a3, a4, a5] }
        //
        // The following structures will be created in the exact order as showed:
        // `block_ranges`: [
        //    (block0, block8, 0 addresses),
        //    (block9, block19, 2 addresses),
        //    (block20, to_block, 5 addresses)
        //  ]
        // `filtered_addresses`: [a1, a2, a3, a4, a5]
        //
        // The first range will delete all receipts between block0 - block8
        // The second range will delete all receipts between block9 - 19, except the ones with
        //     emitter logs from these addresses: [a1, a2].
        // The third range will delete all receipts between block20 - to_block, except the ones with
        //     emitter logs from these addresses: [a1, a2, a3, a4, a5]
        let mut block_ranges = vec![];
        let mut blocks_iter = address_filter.iter().peekable();
        let mut filtered_addresses = vec![];

        while let Some((start_block, addresses)) = blocks_iter.next() {
            filtered_addresses.extend_from_slice(addresses);

            // This will clear all receipts before the first  appearance of a contract log
            if block_ranges.is_empty() {
                block_ranges.push((0, *start_block - 1, 0));
            }

            let end_block =
                blocks_iter.peek().map(|(next_block, _)| *next_block - 1).unwrap_or(to_block);

            // Addresses in lower block ranges, are still included in the inclusion list for future
            // ranges.
            block_ranges.push((*start_block, end_block, filtered_addresses.len()));
        }

        for (start_block, end_block, num_addresses) in block_ranges {
            let range = match self.get_next_tx_num_range_from_checkpoint(
                provider,
                PrunePart::ContractLogs,
                end_block,
            )? {
                Some(range) => range,
                None => {
                    trace!(
                    target: "pruner",
                    block_range = format!("{start_block}..={end_block}"),
                    "No receipts to prune."
                    );
                    continue
                }
            };

            let total = range.clone().count();
            let mut processed = 0;

            provider.prune_table_with_iterator_in_batches::<tables::Receipts>(
                range,
                self.batch_sizes.receipts,
                |rows| {
                    processed += rows;
                    trace!(
                        target: "pruner",
                        %rows,
                        block_range = format!("{start_block}..={end_block}"),
                        progress = format!("{:.1}%", 100.0 * processed as f64 / total as f64),
                        "Pruned receipts"
                    );
                },
                |receipt| {
                    num_addresses > 0 &&
                        receipt.logs.iter().any(|log| {
                            filtered_addresses[..num_addresses].contains(&&log.address)
                        })
                },
            )?;

            // If this is the last block range, avoid writing an unused checkpoint
            if end_block != to_block {
                // This allows us to query for the transactions in the next block range with
                // [`get_next_tx_num_range_from_checkpoint`]. It's just a temporary intermediate
                // checkpoint, which should be adjusted in the end.
                provider.save_prune_checkpoint(
                    PrunePart::ContractLogs,
                    PruneCheckpoint {
                        block_number: end_block,
                        prune_mode: PruneMode::Before(end_block + 1),
                    },
                )?;
            }
        }

        // If there are contracts using `PruneMode::Distance(_)` there will be receipts before
        // `to_block` that become eligible to be pruned in future runs. Therefore, our
        // checkpoint is not actually `to_block`, but the `lowest_block_with_distance` from any
        // contract. This ensures that in future pruner runs we can
        // prune all these receipts between the previous `lowest_block_with_distance` and the new
        // one using `get_next_tx_num_range_from_checkpoint`.
        let checkpoint_block = self
            .modes
            .contract_logs_filter
            .lowest_block_with_distance(tip_block_number, pruned)?
            .unwrap_or(to_block);

        provider.save_prune_checkpoint(
            PrunePart::ContractLogs,
            PruneCheckpoint {
                block_number: checkpoint_block - 1,
                prune_mode: PruneMode::Before(checkpoint_block),
            },
        )?;

        Ok(())
    }

    /// Prune transaction lookup entries up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_transaction_lookup(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let range = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::TransactionLookup,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction lookup entries to prune");
                return Ok(())
            }
        };
        let last_tx_num = *range.end();
        let total = range.clone().count();
        let mut processed = 0;

        for i in range.step_by(self.batch_sizes.transaction_lookup) {
            // The `min` ensures that the transaction range doesn't exceed the last transaction
            // number. `last_tx_num + 1` is used to include the last transaction in the range.
            let tx_range = i..(i + self.batch_sizes.transaction_lookup as u64).min(last_tx_num + 1);

            // Retrieve transactions in the range and calculate their hashes in parallel
            let mut hashes = provider
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

            // Pre-sort hashes to prune them in order
            hashes.sort_unstable();

            let rows = provider.prune_table_with_iterator::<tables::TxHashNumber>(hashes)?;
            processed += rows;
            trace!(
                target: "pruner",
                %rows,
                progress = format!("{:.1}%", 100.0 * processed as f64 / total as f64),
                "Pruned transaction lookup"
            );
        }

        provider.save_prune_checkpoint(
            PrunePart::TransactionLookup,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }

    /// Prune transaction senders up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_transaction_senders(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let range = match self.get_next_tx_num_range_from_checkpoint(
            provider,
            PrunePart::SenderRecovery,
            to_block,
        )? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction senders to prune");
                return Ok(())
            }
        };
        let total = range.clone().count();

        provider.prune_table_with_range_in_batches::<tables::TxSenders>(
            range,
            self.batch_sizes.transaction_senders,
            |rows, _| {
                trace!(
                    target: "pruner",
                    %rows,
                    progress = format!("{:.1}%", 100.0 * rows as f64 / total as f64),
                    "Pruned transaction senders"
                );
            },
        )?;

        provider.save_prune_checkpoint(
            PrunePart::SenderRecovery,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }

    /// Prune account history up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_account_history(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let from_block = provider
            .get_prune_checkpoint(PrunePart::AccountHistory)?
            .map(|checkpoint| checkpoint.block_number + 1)
            .unwrap_or_default();
        let range = from_block..=to_block;
        let total = range.clone().count();

        provider.prune_table_with_range_in_batches::<tables::AccountChangeSet>(
            range,
            self.batch_sizes.account_history,
            |keys, rows| {
                trace!(
                    target: "pruner",
                    %keys,
                    %rows,
                    progress = format!("{:.1}%", 100.0 * keys as f64 / total as f64),
                    "Pruned account history (changesets)"
                );
            },
        )?;

        self.prune_history_indices::<tables::AccountHistory, _>(
            provider,
            to_block,
            |a, b| a.key == b.key,
            |key| ShardedKey::last(key.key),
            self.batch_sizes.account_history,
            |rows| {
                trace!(
                    target: "pruner",
                    rows,
                    "Pruned account history (indices)"
                );
            },
        )?;

        provider.save_prune_checkpoint(
            PrunePart::AccountHistory,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }

    /// Prune storage history up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_storage_history(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let from_block = provider
            .get_prune_checkpoint(PrunePart::StorageHistory)?
            .map(|checkpoint| checkpoint.block_number + 1)
            .unwrap_or_default();
        let block_range = from_block..=to_block;
        let range = BlockNumberAddress::range(block_range);

        provider.prune_table_with_range_in_batches::<tables::StorageChangeSet>(
            range,
            self.batch_sizes.storage_history,
            |keys, rows| {
                trace!(
                    target: "pruner",
                    %keys,
                    %rows,
                    "Pruned storage history (changesets)"
                );
            },
        )?;

        self.prune_history_indices::<tables::StorageHistory, _>(
            provider,
            to_block,
            |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
            |key| StorageShardedKey::last(key.address, key.sharded_key.key),
            self.batch_sizes.storage_history,
            |rows| {
                trace!(
                    target: "pruner",
                    rows,
                    "Pruned storage history (indices)"
                );
            },
        )?;

        provider.save_prune_checkpoint(
            PrunePart::StorageHistory,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }

    /// Prune history indices up to the provided block, inclusive.
    fn prune_history_indices<T, SK>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        key_matches: impl Fn(&T::Key, &T::Key) -> bool,
        last_key: impl Fn(&T::Key) -> T::Key,
        batch_size: usize,
        batch_callback: impl Fn(usize),
    ) -> PrunerResult
    where
        T: Table<Value = BlockNumberList>,
        T::Key: AsRef<ShardedKey<SK>>,
    {
        let mut processed = 0;
        let mut cursor = provider.tx_ref().cursor_write::<T>()?;

        // Prune history table:
        // 1. If the shard has `highest_block_number` less than or equal to the target block number
        // for pruning, delete the shard completely.
        // 2. If the shard has `highest_block_number` greater than the target block number for
        // pruning, filter block numbers inside the shard which are less than the target
        // block number for pruning.
        while let Some(result) = cursor.next()? {
            let (key, blocks): (T::Key, BlockNumberList) = result;

            // If shard consists only of block numbers less than the target one, delete shard
            // completely.
            if key.as_ref().highest_block_number <= to_block {
                cursor.delete_current()?;
                if key.as_ref().highest_block_number == to_block {
                    // Shard contains only block numbers up to the target one, so we can skip to
                    // the last shard for this key. It is guaranteed that further shards for this
                    // sharded key will not contain the target block number, as it's in this shard.
                    cursor.seek_exact(last_key(&key))?;
                }
            }
            // Shard contains block numbers that are higher than the target one, so we need to
            // filter it. It is guaranteed that further shards for this sharded key will not
            // contain the target block number, as it's in this shard.
            else {
                let new_blocks = blocks
                    .iter(0)
                    .skip_while(|block| *block <= to_block as usize)
                    .collect::<Vec<_>>();

                // If there were blocks less than or equal to the target one
                // (so the shard has changed), update the shard.
                if blocks.len() != new_blocks.len() {
                    // If there are no more blocks in this shard, we need to remove it, as empty
                    // shards are not allowed.
                    if new_blocks.is_empty() {
                        if key.as_ref().highest_block_number == u64::MAX {
                            let prev_row = cursor.prev()?;
                            match prev_row {
                                // If current shard is the last shard for the sharded key that
                                // has previous shards, replace it with the previous shard.
                                Some((prev_key, prev_value)) if key_matches(&prev_key, &key) => {
                                    cursor.delete_current()?;
                                    // Upsert will replace the last shard for this sharded key with
                                    // the previous value.
                                    cursor.upsert(key.clone(), prev_value)?;
                                }
                                // If there's no previous shard for this sharded key,
                                // just delete last shard completely.
                                _ => {
                                    // If we successfully moved the cursor to a previous row,
                                    // jump to the original last shard.
                                    if prev_row.is_some() {
                                        cursor.next()?;
                                    }
                                    // Delete shard.
                                    cursor.delete_current()?;
                                }
                            }
                        }
                        // If current shard is not the last shard for this sharded key,
                        // just delete it.
                        else {
                            cursor.delete_current()?;
                        }
                    } else {
                        cursor.upsert(key.clone(), BlockNumberList::new_pre_sorted(new_blocks))?;
                    }
                }

                // Jump to the last shard for this key, if current key isn't already the last shard.
                if key.as_ref().highest_block_number != u64::MAX {
                    cursor.seek_exact(last_key(&key))?;
                }
            }

            processed += 1;

            if processed % batch_size == 0 {
                batch_callback(batch_size);
            }
        }

        if processed % batch_size != 0 {
            batch_callback(processed % batch_size);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{pruner::BatchSizes, Pruner};
    use assert_matches::assert_matches;
    use reth_db::{tables, test_utils::create_test_rw_db, BlockNumberList};
    use reth_interfaces::test_utils::{
        generators,
        generators::{
            random_block_range, random_changeset_range, random_eoa_account_range, random_receipt,
        },
    };
    use reth_primitives::{
        BlockNumber, PruneCheckpoint, PruneMode, PruneModes, PrunePart, H256, MAINNET,
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

            let provider = tx.inner_rw();
            assert_matches!(pruner.prune_receipts(&provider, to_block, prune_mode), Ok(()));
            provider.commit().expect("commit");

            assert_eq!(
                tx.table::<tables::Receipts>().unwrap().len(),
                blocks[to_block as usize + 1..].iter().map(|block| block.body.len()).sum::<usize>()
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::Receipts).unwrap(),
                Some(PruneCheckpoint { block_number: to_block, prune_mode })
            );
        };

        // Pruning first time ever, no previous checkpoint is present
        test_prune(10);
        // Prune second time, previous checkpoint is present, should continue pruning from where
        // ended last time
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

            let provider = tx.inner_rw();
            assert_matches!(
                pruner.prune_transaction_lookup(&provider, to_block, prune_mode),
                Ok(())
            );
            provider.commit().expect("commit");

            assert_eq!(
                tx.table::<tables::TxHashNumber>().unwrap().len(),
                blocks[to_block as usize + 1..].iter().map(|block| block.body.len()).sum::<usize>()
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::TransactionLookup).unwrap(),
                Some(PruneCheckpoint { block_number: to_block, prune_mode })
            );
        };

        // Pruning first time ever, no previous checkpoint is present
        test_prune(10);
        // Prune second time, previous checkpoint is present, should continue pruning from where
        // ended last time
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

            let provider = tx.inner_rw();
            assert_matches!(
                pruner.prune_transaction_senders(&provider, to_block, prune_mode),
                Ok(())
            );
            provider.commit().expect("commit");

            assert_eq!(
                tx.table::<tables::TxSenders>().unwrap().len(),
                blocks[to_block as usize + 1..].iter().map(|block| block.body.len()).sum::<usize>()
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::SenderRecovery).unwrap(),
                Some(PruneCheckpoint { block_number: to_block, prune_mode })
            );
        };

        // Pruning first time ever, no previous checkpoint is present
        test_prune(10);
        // Prune second time, previous checkpoint is present, should continue pruning from where
        // ended last time
        test_prune(20);
    }

    #[test]
    fn prune_account_history() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let block_num = 7000;
        let blocks = random_block_range(&mut rng, 0..=block_num, H256::zero(), 0..1);
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

        let test_prune = |to_block: BlockNumber| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { account_history: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    account_history: 10,
                    ..Default::default()
                },
            );

            let provider = tx.inner_rw();
            assert_matches!(pruner.prune_account_history(&provider, to_block, prune_mode), Ok(()));
            provider.commit().expect("commit");

            assert_eq!(
                tx.table::<tables::AccountChangeSet>().unwrap().len(),
                changesets[to_block as usize + 1..].iter().flatten().count()
            );

            let actual_shards = tx.table::<tables::AccountHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.highest_block_number > to_block)
                .map(|(key, blocks)| {
                    let new_blocks = blocks
                        .iter(0)
                        .skip_while(|block| *block <= to_block as usize)
                        .collect::<Vec<_>>();
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::AccountHistory).unwrap(),
                Some(PruneCheckpoint { block_number: to_block, prune_mode })
            );
        };

        // Prune first time: no previous checkpoint is present
        test_prune(3000);
        // Prune second time: previous checkpoint is present, should continue pruning from where
        // ended last time
        test_prune(4500);
    }

    #[test]
    fn prune_storage_history() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let block_num = 7000;
        let blocks = random_block_range(&mut rng, 0..=block_num, H256::zero(), 0..1);
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

        let test_prune = |to_block: BlockNumber| {
            let prune_mode = PruneMode::Before(to_block);
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes { storage_history: Some(prune_mode), ..Default::default() },
                BatchSizes {
                    // Less than total amount of blocks to prune to test the batching logic
                    storage_history: 10,
                    ..Default::default()
                },
            );

            let provider = tx.inner_rw();
            assert_matches!(pruner.prune_storage_history(&provider, to_block, prune_mode), Ok(()));
            provider.commit().expect("commit");

            assert_eq!(
                tx.table::<tables::StorageChangeSet>().unwrap().len(),
                changesets[to_block as usize + 1..]
                    .iter()
                    .flatten()
                    .flat_map(|(_, _, entries)| entries)
                    .count()
            );

            let actual_shards = tx.table::<tables::StorageHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.sharded_key.highest_block_number > to_block)
                .map(|(key, blocks)| {
                    let new_blocks = blocks
                        .iter(0)
                        .skip_while(|block| *block <= to_block as usize)
                        .collect::<Vec<_>>();
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                tx.inner().get_prune_checkpoint(PrunePart::StorageHistory).unwrap(),
                Some(PruneCheckpoint { block_number: to_block, prune_mode })
            );
        };

        // Prune first time: no previous checkpoint is present
        test_prune(3000);
        // Prune second time: previous checkpoint is present, should continue pruning from where
        // ended last time
        test_prune(4500);
    }
}
