use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{database::Database, tables};
use reth_primitives::{PruneCheckpoint, PruneMode, PruneProgress, PruneSegment};
use reth_provider::{
    errors::provider::ProviderResult, DatabaseProviderRW, PruneCheckpointWriter,
    TransactionsProvider,
};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct Receipts {
    mode: PruneMode,
}

impl Receipts {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for Receipts {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Receipts
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        let tx_range = match input.get_next_tx_num_range(provider)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No receipts to prune");
                return Ok(PruneOutput::done())
            }
        };
        let tx_range_end = *tx_range.end();

        let mut limiter = input.limiter;

        let mut last_pruned_transaction = tx_range_end;
        let (pruned, done) = provider.prune_table_with_range::<tables::Receipts>(
            tx_range,
            &mut limiter,
            |_| false,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %pruned, %done, "Pruned receipts");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more receipts to prune, set the checkpoint block number to previous,
            // so we could finish pruning its receipts on the next run.
            .checked_sub(if done { 0 } else { 1 });

        let progress = PruneProgress::new(done, &limiter);

        Ok(PruneOutput {
            progress,
            pruned,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
            }),
        })
    }

    fn save_checkpoint(
        &self,
        provider: &DatabaseProviderRW<DB>,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        provider.save_prune_checkpoint(PruneSegment::Receipts, checkpoint)?;

        // `PruneSegment::Receipts` overrides `PruneSegment::ContractLogs`, so we can preemptively
        // limit their pruning start point.
        provider.save_prune_checkpoint(PruneSegment::ContractLogs, checkpoint)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, PruneOutput, Receipts, Segment};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_primitives::{
        BlockNumber, PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress,
        PruneSegment, TxNumber, B256,
    };
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::{
        generators,
        generators::{random_block_range, random_receipt},
    };
    use std::ops::Sub;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 1..=10, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                receipts
                    .push((receipts.len() as u64, random_receipt(&mut rng, transaction, Some(0))));
            }
        }
        db.insert_receipts(receipts.clone()).expect("insert receipts");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::Receipts>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let segment = Receipts::new(prune_mode);
            let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Receipts)
                    .unwrap(),
                to_block,
                limiter: limiter.clone(),
            };

            let next_tx_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::Receipts)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number = blocks
                .iter()
                .take(to_block as usize)
                .map(|block| block.body.len())
                .sum::<usize>()
                .min(
                    next_tx_number_to_prune as usize +
                        input.limiter.deleted_entries_limit().unwrap(),
                )
                .sub(1);

            let provider = db.factory.provider_rw().unwrap();
            let result = segment.prune(&provider, input).unwrap();
            limiter.increment_deleted_entries_count_by(result.pruned);

            assert_matches!(
                result,
                PruneOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

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
                .0
                .checked_sub(if result.progress.is_finished() { 0 } else { 1 });

            assert_eq!(
                db.table::<tables::Receipts>().unwrap().len(),
                receipts.len() - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Receipts)
                    .unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(
            6,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 10),
        );
        test_prune(6, (PruneProgress::Finished, 2));
        test_prune(10, (PruneProgress::Finished, 8));
    }
}
