use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db::{tables, transaction::DbTxMut};
use reth_provider::{providers::StaticFileProvider, BlockReader, DBProvider, TransactionsProvider};
use reth_prune_types::{
    PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use tracing::trace;

#[derive(Debug)]
pub struct Transactions {
    static_file_provider: StaticFileProvider,
}

impl Transactions {
    pub const fn new(static_file_provider: StaticFileProvider) -> Self {
        Self { static_file_provider }
    }
}

impl<Provider> Segment<Provider> for Transactions
where
    Provider: DBProvider<Tx: DbTxMut> + TransactionsProvider + BlockReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::Transactions
    }

    fn mode(&self) -> Option<PruneMode> {
        self.static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Transactions)
            .map(PruneMode::before_inclusive)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::StaticFile
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let tx_range = match input.get_next_tx_num_range(provider)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transactions to prune");
                return Ok(SegmentOutput::done())
            }
        };

        let mut limiter = input.limiter;

        let mut last_pruned_transaction = *tx_range.end();
        let (pruned, done) = provider.tx_ref().prune_table_with_range::<tables::Transactions>(
            tx_range,
            &mut limiter,
            |_| false,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %pruned, %done, "Pruned transactions");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transactions to prune, set the checkpoint block number to previous,
            // so we could finish pruning its transactions on the next run.
            .checked_sub(if done { 0 } else { 1 });

        let progress = PruneProgress::new(done, &limiter);

        Ok(SegmentOutput {
            progress,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, Segment};
    use alloy_primitives::{BlockNumber, TxNumber, B256};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_provider::{
        DatabaseProviderFactory, PruneCheckpointReader, PruneCheckpointWriter,
        StaticFileProviderFactory,
    };
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress,
        PruneSegment, SegmentOutput,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
    use std::ops::Sub;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=100,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let transactions =
            blocks.iter().flat_map(|block| &block.body.transactions).collect::<Vec<_>>();

        assert_eq!(db.table::<tables::Transactions>().unwrap().len(), transactions.len());

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let segment = super::Transactions::new(db.factory.static_file_provider());
            let prune_mode = PruneMode::Before(to_block);
            let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Transactions)
                    .unwrap(),
                to_block,
                limiter: limiter.clone(),
            };

            let next_tx_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::Transactions)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let provider = db.factory.database_provider_rw().unwrap();
            let result = segment.prune(&provider, input.clone()).unwrap();
            limiter.increment_deleted_entries_count_by(result.pruned);

            assert_matches!(
                result,
                SegmentOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            provider
                .save_prune_checkpoint(
                    PruneSegment::Transactions,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            let last_pruned_tx_number = blocks
                .iter()
                .take(to_block as usize)
                .map(|block| block.body.transactions.len())
                .sum::<usize>()
                .min(
                    next_tx_number_to_prune as usize +
                        input.limiter.deleted_entries_limit().unwrap(),
                )
                .sub(1);

            let last_pruned_block_number = blocks
                .iter()
                .fold_while((0, 0), |(_, mut tx_count), block| {
                    tx_count += block.body.transactions.len();

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
                db.table::<tables::Transactions>().unwrap().len(),
                transactions.len() - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Transactions)
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
    }
}
