use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment, SegmentOutput},
    PrunerError,
};
use rayon::prelude::*;
use reth_db::{tables, transaction::DbTxMut};
use reth_provider::{BlockReader, DBProvider, TransactionsProvider};
use reth_prune_types::{
    PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutputCheckpoint,
};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct TransactionLookup {
    mode: PruneMode,
}

impl TransactionLookup {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for TransactionLookup
where
    Provider: DBProvider<Tx: DbTxMut> + TransactionsProvider + BlockReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::TransactionLookup
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let (start, end) = match input.get_next_tx_num_range(provider)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction lookup entries to prune");
                return Ok(SegmentOutput::done())
            }
        }
        .into_inner();
        let tx_range = start..=
            Some(end)
                .min(input.limiter.deleted_entries_limit_left().map(|left| start + left as u64 - 1))
                .unwrap();
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

        let mut limiter = input.limiter;

        let mut last_pruned_transaction = None;
        let (pruned, done) =
            provider.tx_ref().prune_table_with_iterator::<tables::TransactionHashNumbers>(
                hashes,
                &mut limiter,
                |row| {
                    last_pruned_transaction =
                        Some(last_pruned_transaction.unwrap_or(row.1).max(row.1))
                },
            )?;

        let done = done && tx_range_end == end;
        trace!(target: "pruner", %pruned, %done, "Pruned transaction lookup");

        let last_pruned_transaction = last_pruned_transaction.unwrap_or(tx_range_end);

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction lookup entries to prune, set the checkpoint block number
            // to previous, so we could finish pruning its transaction lookup entries on the next
            // run.
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
    use crate::segments::{PruneInput, Segment, SegmentOutput, TransactionLookup};
    use alloy_primitives::{BlockNumber, TxNumber, B256};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_provider::{DatabaseProviderFactory, PruneCheckpointReader};
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress, PruneSegment,
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
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            for transaction in &block.body.transactions {
                tx_hash_numbers.push((transaction.hash, tx_hash_numbers.len() as u64));
            }
        }
        db.insert_tx_hash_numbers(tx_hash_numbers.clone()).expect("insert tx hash numbers");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.transactions.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::TransactionHashNumbers>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let segment = TransactionLookup::new(prune_mode);
            let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::TransactionLookup)
                    .unwrap(),
                to_block,
                limiter: limiter.clone(),
            };

            let next_tx_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::TransactionLookup)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

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
                .0;

            let provider = db.factory.database_provider_rw().unwrap();
            let result = segment.prune(&provider, input).unwrap();
            limiter.increment_deleted_entries_count_by(result.pruned);

            assert_matches!(
                result,
                SegmentOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number = last_pruned_block_number
                .checked_sub(if result.progress.is_finished() { 0 } else { 1 });

            assert_eq!(
                db.table::<tables::TransactionHashNumbers>().unwrap().len(),
                tx_hash_numbers.len() - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::TransactionLookup)
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
