use crate::{
    segments::{PruneInput, PruneOutput, Segment},
    PrunerError,
};
use reth_db::{database::Database, tables};
use reth_primitives::{
    PruneCheckpoint, PruneMode, PruneSegment, ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
use reth_provider::{BlockReader, DatabaseProviderRW, PruneCheckpointWriter, TransactionsProvider};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct ReceiptsByLogs {
    config: ReceiptsLogPruneConfig,
}

impl ReceiptsByLogs {
    pub fn new(config: ReceiptsLogPruneConfig) -> Self {
        Self { config }
    }
}

impl<DB: Database> Segment<DB> for ReceiptsByLogs {
    fn segment(&self) -> PruneSegment {
        PruneSegment::ContractLogs
    }

    fn mode(&self) -> Option<PruneMode> {
        None
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        // Contract log filtering removes every receipt possible except the ones in the list. So,
        // for the other receipts it's as if they had a `PruneMode::Distance()` of
        // `MINIMUM_PRUNING_DISTANCE`.
        let to_block = PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)
            .prune_target_block(input.to_block, PruneSegment::ContractLogs)?
            .map(|(bn, _)| bn)
            .unwrap_or_default();

        // Get status checkpoint from latest run
        let mut last_pruned_block =
            input.previous_checkpoint.and_then(|checkpoint| checkpoint.block_number);

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
        let address_filter = self.config.group_by_block(input.to_block, last_pruned_block)?;

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

        let mut limit = input.delete_limit;
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
        // checkpoint is the `last_pruned_block`.
        let prune_mode_block = self
            .config
            .lowest_block_with_distance(input.to_block, initial_last_pruned_block)?
            .unwrap_or(to_block);

        provider.save_prune_checkpoint(
            PruneSegment::ContractLogs,
            PruneCheckpoint {
                block_number: Some(prune_mode_block.min(last_pruned_block.unwrap_or(u64::MAX))),
                tx_number: last_pruned_transaction,
                prune_mode: PruneMode::Before(prune_mode_block),
            },
        )?;

        Ok(PruneOutput { done, pruned: input.delete_limit - limit, checkpoint: None })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{receipts_by_logs::ReceiptsByLogs, PruneInput, Segment};
    use assert_matches::assert_matches;
    use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_eoa_account, random_log, random_receipt},
    };
    use reth_primitives::{PruneMode, PruneSegment, ReceiptsLogPruneConfig, B256};
    use reth_provider::{PruneCheckpointReader, TransactionsProvider};
    use reth_stages::test_utils::TestStageDB;
    use std::collections::BTreeMap;

    #[test]
    fn prune_receipts_by_logs() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let tip = 20000;
        let blocks = [
            random_block_range(&mut rng, 0..=100, B256::ZERO, 1..5),
            random_block_range(&mut rng, (100 + 1)..=(tip - 100), B256::ZERO, 0..1),
            random_block_range(&mut rng, (tip - 100 + 1)..=tip, B256::ZERO, 1..5),
        ]
        .concat();
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut receipts = Vec::new();

        let (deposit_contract_addr, _) = random_eoa_account(&mut rng);
        for block in &blocks {
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
        db.insert_receipts(receipts).expect("insert receipts");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::Receipts>().unwrap().len()
        );

        let run_prune = || {
            let provider = db.factory.provider_rw().unwrap();

            let prune_before_block: usize = 20;
            let prune_mode = PruneMode::Before(prune_before_block as u64);
            let receipts_log_filter =
                ReceiptsLogPruneConfig(BTreeMap::from([(deposit_contract_addr, prune_mode)]));

            let result = ReceiptsByLogs::new(receipts_log_filter).prune(
                &provider,
                PruneInput {
                    previous_checkpoint: db
                        .factory
                        .provider()
                        .unwrap()
                        .get_prune_checkpoint(PruneSegment::ContractLogs)
                        .unwrap(),
                    to_block: tip,
                    delete_limit: 10,
                },
            );
            provider.commit().expect("commit");

            assert_matches!(result, Ok(_));
            let output = result.unwrap();

            let (pruned_block, pruned_tx) = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::ContractLogs)
                .unwrap()
                .map(|checkpoint| (checkpoint.block_number.unwrap(), checkpoint.tx_number.unwrap()))
                .unwrap_or_default();

            // All receipts are in the end of the block
            let unprunable = pruned_block.saturating_sub(prune_before_block as u64 - 1);

            assert_eq!(
                db.table::<tables::Receipts>().unwrap().len(),
                blocks.iter().map(|block| block.body.len()).sum::<usize>() -
                    ((pruned_tx + 1) - unprunable) as usize
            );

            output.done
        };

        while !run_prune() {}

        let provider = db.factory.provider().unwrap();
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
