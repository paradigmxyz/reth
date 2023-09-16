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
    BlockNumber, ChainSpec, PruneBatchSizes, PruneCheckpoint, PruneMode, PruneModes, PrunePart,
    TxNumber, MINIMUM_PRUNING_DISTANCE,
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

/// Pruning routine. Main pruning logic happens in [Pruner::run].
pub struct Pruner<DB> {
    metrics: Metrics,
    provider_factory: ProviderFactory<DB>,
    /// Minimum pruning interval measured in blocks. All prune parts are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: usize,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
    modes: PruneModes,
    /// Maximum entries to prune per block, per prune part.
    batch_sizes: PruneBatchSizes,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        min_block_interval: usize,
        modes: PruneModes,
        batch_sizes: PruneBatchSizes,
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
        if tip_block_number == 0 {
            self.last_pruned_block_number = Some(tip_block_number);

            trace!(target: "pruner", %tip_block_number, "Nothing to prune yet");
            return Ok(true)
        }

        trace!(target: "pruner", %tip_block_number, "Pruner started");
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
            trace!(target: "pruner", prune_part = ?PrunePart::Receipts, "No target block to prune");
        }

        if !self.modes.receipts_log_filter.is_empty() {
            let part_start = Instant::now();
            let part_done = self.prune_receipts_by_logs(&provider, tip_block_number)?;
            done = done && part_done;
            self.metrics
                .get_prune_part_metrics(PrunePart::ContractLogs)
                .duration_seconds
                .record(part_start.elapsed())
        } else {
            trace!(target: "pruner", prune_part = ?PrunePart::ContractLogs, "No filter to prune");
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

        trace!(target: "pruner", %tip_block_number, ?elapsed, "Pruner finished");
        Ok(done)
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        if self.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previously pruned block number. If
            // that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(last_pruned_block_number) >=
                self.min_block_interval as u64
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

        let range = from_tx_number..=to_tx_number;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
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
            self.batch_sizes.receipts(self.min_block_interval),
            |_| false,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %deleted, %done, "Pruned receipts");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more receipts to prune, set the checkpoint block number to previous,
            // so we could finish pruning its receipts on the next run.
            .checked_sub(if done { 0 } else { 1 });

        let prune_checkpoint = PruneCheckpoint {
            block_number: last_pruned_block,
            tx_number: Some(last_pruned_transaction),
            prune_mode,
        };

        provider.save_prune_checkpoint(PrunePart::Receipts, prune_checkpoint)?;

        // `PrunePart::Receipts` overrides `PrunePart::ContractLogs`, so we can preemptively
        // limit their pruning start point.
        provider.save_prune_checkpoint(PrunePart::ContractLogs, prune_checkpoint)?;

        Ok(done)
    }

    /// Prune receipts up to the provided block, inclusive, by filtering logs. Works as in inclusion
    /// list, and removes every receipt not belonging to it. Respects the batch size.
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

        // Get status checkpoint from latest run
        let mut last_pruned_block = provider
            .get_prune_checkpoint(PrunePart::ContractLogs)?
            .and_then(|checkpoint| checkpoint.block_number);

        let initial_last_pruned_block = last_pruned_block;

        let mut from_tx_number = match initial_last_pruned_block {
            Some(block) => provider
                .block_body_indices(block)?
                .map(|block| block.last_tx_num() + 1)
                .unwrap_or(0),
            None => 0,
        };

        // Figure out what receipts have already been pruned, so we can have an accurate
        // `address_filter`
        let address_filter =
            self.modes.receipts_log_filter.group_by_block(tip_block_number, last_pruned_block)?;

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

            // This will clear all receipts before the first  appearance of a contract log or since
            // the block after the last pruned one.
            if block_ranges.is_empty() {
                let init = last_pruned_block.map(|b| b + 1).unwrap_or_default();
                if init < *start_block {
                    block_ranges.push((init, *start_block - 1, 0));
                }
            }

            let end_block =
                blocks_iter.peek().map(|(next_block, _)| *next_block - 1).unwrap_or(to_block);

            // Addresses in lower block ranges, are still included in the inclusion list for future
            // ranges.
            block_ranges.push((*start_block, end_block, filtered_addresses.len()));
        }

        trace!(
            target: "pruner",
            ?block_ranges,
            ?filtered_addresses,
            "Calculated block ranges and filtered addresses",
        );

        let mut limit = self.batch_sizes.receipts(self.min_block_interval);
        let mut done = true;
        let mut last_pruned_transaction = None;
        for (start_block, end_block, num_addresses) in block_ranges {
            let block_range = start_block..=end_block;

            // Calculate the transaction range from this block range
            let tx_range_end = match provider.block_body_indices(end_block)? {
                Some(body) => body.last_tx_num(),
                None => {
                    trace!(
                        target: "pruner",
                        ?block_range,
                        "No receipts to prune."
                    );
                    continue
                }
            };
            let tx_range = from_tx_number..=tx_range_end;

            // Delete receipts, except the ones in the inclusion list
            let mut last_skipped_transaction = 0;
            let deleted;
            (deleted, done) = provider.prune_table_with_range::<tables::Receipts>(
                tx_range,
                limit,
                |(tx_num, receipt)| {
                    let skip = num_addresses > 0 &&
                        receipt.logs.iter().any(|log| {
                            filtered_addresses[..num_addresses].contains(&&log.address)
                        });

                    if skip {
                        last_skipped_transaction = *tx_num;
                    }
                    skip
                },
                |row| last_pruned_transaction = Some(row.0),
            )?;
            trace!(target: "pruner", %deleted, %done, ?block_range, "Pruned receipts");

            limit = limit.saturating_sub(deleted);

            // For accurate checkpoints we need to know that we have checked every transaction.
            // Example: we reached the end of the range, and the last receipt is supposed to skip
            // its deletion.
            last_pruned_transaction =
                Some(last_pruned_transaction.unwrap_or_default().max(last_skipped_transaction));
            last_pruned_block = Some(
                provider
                    .transaction_block(last_pruned_transaction.expect("qed"))?
                    .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
                    // If there's more receipts to prune, set the checkpoint block number to
                    // previous, so we could finish pruning its receipts on the
                    // next run.
                    .saturating_sub(if done { 0 } else { 1 }),
            );

            if limit == 0 {
                done &= end_block == to_block;
                break
            }

            from_tx_number = last_pruned_transaction.expect("qed") + 1;
        }

        // If there are contracts using `PruneMode::Distance(_)` there will be receipts before
        // `to_block` that become eligible to be pruned in future runs. Therefore, our checkpoint is
        // not actually `to_block`, but the `lowest_block_with_distance` from any contract.
        // This ensures that in future pruner runs we can prune all these receipts between the
        // previous `lowest_block_with_distance` and the new one using
        // `get_next_tx_num_range_from_checkpoint`.
        //
        // Only applies if we were able to prune everything intended for this run, otherwise the
        // checkpoing is the `last_pruned_block`.
        let prune_mode_block = self
            .modes
            .receipts_log_filter
            .lowest_block_with_distance(tip_block_number, initial_last_pruned_block)?
            .unwrap_or(to_block);

        provider.save_prune_checkpoint(
            PrunePart::ContractLogs,
            PruneCheckpoint {
                block_number: Some(prune_mode_block.min(last_pruned_block.unwrap_or(u64::MAX))),
                tx_number: last_pruned_transaction,
                prune_mode: PruneMode::Before(prune_mode_block),
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
        let tx_range = start..=
            (end.min(
                start + self.batch_sizes.transaction_lookup(self.min_block_interval) as u64 - 1,
            ));
        let tx_range_end = *tx_range.end();

        // Retrieve transactions in the range and calculate their hashes in parallel
        let hashes = provider
            .transactions_by_tx_range(tx_range.clone())?
            .into_par_iter()
            .map(|transaction| transaction.hash())
            .collect::<Vec<_>>();

        // Number of transactions retrieved from the database should match the tx range count
        let tx_count = tx_range.count();
        if hashes.len() != tx_count {
            return Err(PrunerError::InconsistentData(
                "Unexpected number of transaction hashes retrieved by transaction number range",
            ))
        }

        let mut last_pruned_transaction = tx_range_end;
        let (deleted, done) = provider.prune_table_with_iterator::<tables::TxHashNumber>(
            hashes,
            self.batch_sizes.transaction_lookup(self.min_block_interval),
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
            self.batch_sizes.transaction_senders(self.min_block_interval),
            |_| false,
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

        let mut last_changeset_pruned_block = None;
        let (rows, done) = provider.prune_table_with_range::<tables::AccountChangeSet>(
            range,
            self.batch_sizes.account_history(self.min_block_interval),
            |_| false,
            |row| last_changeset_pruned_block = Some(row.0),
        )?;
        trace!(target: "pruner", %rows, %done, "Pruned account history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more account account changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its account changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let (processed, deleted) = self.prune_history_indices::<tables::AccountHistory, _>(
            provider,
            last_changeset_pruned_block,
            |a, b| a.key == b.key,
            |key| ShardedKey::last(key.key),
        )?;
        trace!(target: "pruner", %processed, %deleted, %done, "Pruned account history (history)" );

        provider.save_prune_checkpoint(
            PrunePart::AccountHistory,
            PruneCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
                prune_mode,
            },
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

        let mut last_changeset_pruned_block = None;
        let (rows, done) = provider.prune_table_with_range::<tables::StorageChangeSet>(
            BlockNumberAddress::range(range),
            self.batch_sizes.storage_history(self.min_block_interval),
            |_| false,
            |row| last_changeset_pruned_block = Some(row.0.block_number()),
        )?;
        trace!(target: "pruner", %rows, %done, "Pruned storage history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more account storage changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its storage changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let (processed, deleted) = self.prune_history_indices::<tables::StorageHistory, _>(
            provider,
            last_changeset_pruned_block,
            |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
            |key| StorageShardedKey::last(key.address, key.sharded_key.key),
        )?;
        trace!(target: "pruner", %processed, %deleted, %done, "Pruned storage history (history)" );

        provider.save_prune_checkpoint(
            PrunePart::StorageHistory,
            PruneCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
                prune_mode,
            },
        )?;

        Ok(done)
    }

    /// Prune history indices up to the provided block, inclusive.
    ///
    /// Returns total number of processed (walked) and deleted entities.
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

            // If shard consists only of block numbers less than the target one, delete shard
            // completely.
            if key.as_ref().highest_block_number <= to_block {
                cursor.delete_current()?;
                deleted += 1;
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
                                    deleted += 1;
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
                                    deleted += 1;
                                }
                            }
                        }
                        // If current shard is not the last shard for this sharded key,
                        // just delete it.
                        else {
                            cursor.delete_current()?;
                            deleted += 1;
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
        }

        Ok((processed, deleted))
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::{
        cursor::DbCursorRO, tables, test_utils::create_test_rw_db, transaction::DbTx,
        BlockNumberList,
    };
    use reth_interfaces::test_utils::{
        generators,
        generators::{
            random_block_range, random_changeset_range, random_eoa_account,
            random_eoa_account_range, random_log, random_receipt,
        },
    };
    use reth_primitives::{
        BlockNumber, PruneBatchSizes, PruneCheckpoint, PruneMode, PruneModes, PrunePart,
        ReceiptsLogPruneConfig, TxNumber, H256, MAINNET,
    };
    use reth_provider::{PruneCheckpointReader, TransactionsProvider};
    use reth_stages::test_utils::TestTransaction;
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let pruner =
            Pruner::new(db, MAINNET.clone(), 5, PruneModes::none(), PruneBatchSizes::default());

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));

        // Delta is not less than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval as u64;
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
                1,
                PruneModes { receipts: Some(prune_mode), ..Default::default() },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_receipts(10),
            );

            let next_tx_number_to_prune = tx
                .inner()
                .get_prune_checkpoint(PrunePart::Receipts)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number =
                blocks.iter().map(|block| block.body.len()).sum::<usize>().min(
                    next_tx_number_to_prune as usize +
                        pruner.batch_sizes.receipts(pruner.min_block_interval) -
                        1,
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
                1,
                PruneModes { transaction_lookup: Some(prune_mode), ..Default::default() },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_transaction_lookup(10),
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
                    next_tx_number_to_prune as usize +
                        pruner.batch_sizes.transaction_lookup(pruner.min_block_interval) -
                        1,
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
                1,
                PruneModes { sender_recovery: Some(prune_mode), ..Default::default() },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_transaction_senders(10),
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
                    next_tx_number_to_prune as usize +
                        pruner.batch_sizes.transaction_senders(pruner.min_block_interval) -
                        1,
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
                1,
                PruneModes { account_history: Some(prune_mode), ..Default::default() },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_account_history(2000),
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
                    changeset.iter().map(move |change| (block_number, change))
                })
                .collect::<Vec<_>>();

            #[allow(clippy::skip_while_next)]
            let pruned = changesets
                .iter()
                .enumerate()
                .skip_while(|(i, (block_number, _))| {
                    *i < pruner.batch_sizes.account_history(pruner.min_block_interval) * run &&
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

            let pruned_changesets = pruned_changesets.fold(
                BTreeMap::<_, Vec<_>>::new(),
                |mut acc, (block_number, change)| {
                    acc.entry(block_number).or_default().push(change);
                    acc
                },
            );

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
                1,
                PruneModes { storage_history: Some(prune_mode), ..Default::default() },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_storage_history(2000),
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
                    changeset.iter().flat_map(move |(address, _, entries)| {
                        entries.iter().map(move |entry| (block_number, address, entry))
                    })
                })
                .collect::<Vec<_>>();

            #[allow(clippy::skip_while_next)]
            let pruned = changesets
                .iter()
                .enumerate()
                .skip_while(|(i, (block_number, _, _))| {
                    *i < pruner.batch_sizes.storage_history(pruner.min_block_interval) * run &&
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
                BTreeMap::<_, Vec<_>>::new(),
                |mut acc, (block_number, address, entry)| {
                    acc.entry((block_number, address)).or_default().push(entry);
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

    #[test]
    fn prune_receipts_by_logs() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let tip = 300;
        let blocks = random_block_range(&mut rng, 0..=tip, H256::zero(), 1..5);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut receipts = Vec::new();

        let (deposit_contract_addr, _) = random_eoa_account(&mut rng);
        for block in &blocks {
            assert!(!block.body.is_empty());
            for (txi, transaction) in block.body.iter().enumerate() {
                let mut receipt = random_receipt(&mut rng, transaction, Some(1));
                receipt.logs.push(random_log(
                    &mut rng,
                    if txi == (block.body.len() - 1) { Some(deposit_contract_addr) } else { None },
                    Some(1),
                ));
                receipts.push((receipts.len() as u64, receipt));
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

        let run_prune = || {
            let provider = tx.inner_rw();

            let prune_before_block: usize = 20;
            let prune_mode = PruneMode::Before(prune_before_block as u64);
            let receipts_log_filter =
                ReceiptsLogPruneConfig(BTreeMap::from([(deposit_contract_addr, prune_mode)]));
            let pruner = Pruner::new(
                tx.inner_raw(),
                MAINNET.clone(),
                5,
                PruneModes {
                    receipts_log_filter: receipts_log_filter.clone(),
                    ..Default::default()
                },
                // Less than total amount of blocks to prune to test the batching logic
                PruneBatchSizes::default().with_storage_history(10),
            );

            let result = pruner.prune_receipts_by_logs(&provider, tip);
            assert_matches!(result, Ok(_));
            let done = result.unwrap();
            provider.commit().expect("commit");

            let (pruned_block, pruned_tx) = tx
                .inner()
                .get_prune_checkpoint(PrunePart::ContractLogs)
                .unwrap()
                .map(|checkpoint| (checkpoint.block_number.unwrap(), checkpoint.tx_number.unwrap()))
                .unwrap_or_default();

            // All receipts are in the end of the block
            let unprunable = pruned_block.saturating_sub(prune_before_block as u64 - 1);

            assert_eq!(
                tx.table::<tables::Receipts>().unwrap().len(),
                blocks.iter().map(|block| block.body.len()).sum::<usize>() -
                    ((pruned_tx + 1) - unprunable) as usize
            );

            done
        };

        while !run_prune() {}

        let provider = tx.inner();
        let mut cursor = provider.tx_ref().cursor_read::<tables::Receipts>().unwrap();
        let walker = cursor.walk(None).unwrap();
        for receipt in walker {
            let (tx_num, receipt) = receipt.unwrap();

            // Either we only find our contract, or the receipt is part of the unprunable receipts
            // set by tip - 128
            assert!(
                receipt.logs.iter().any(|l| l.address == deposit_contract_addr) ||
                    provider.transaction_block(tx_num).unwrap().unwrap() > tip - 128,
            );
        }
    }
}
