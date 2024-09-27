//! Common receipts pruning logic shared between user and static file pruning segments.
//!
//! - [`crate::segments::user::Receipts`] is responsible for pruning receipts according to the
//!   user-configured settings (for example, on a full node or with a custom prune config)
//! - [`crate::segments::static_file::Receipts`] is responsible for pruning receipts on an archive
//!   node after static file producer has finished

use crate::{db_ext::DbTxPruneExt, segments::PruneInput, PrunerError};
use reth_db::{tables, transaction::DbTxMut};
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DBProvider, PruneCheckpointWriter,
    TransactionsProvider,
};
use reth_prune_types::{
    PruneCheckpoint, PruneProgress, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use tracing::trace;

pub(crate) fn prune<Provider>(
    provider: &Provider,
    input: PruneInput,
) -> Result<SegmentOutput, PrunerError>
where
    Provider: DBProvider<Tx: DbTxMut> + TransactionsProvider + BlockReader,
{
    let tx_range = match input.get_next_tx_num_range(provider)? {
        Some(range) => range,
        None => {
            trace!(target: "pruner", "No receipts to prune");
            return Ok(SegmentOutput::done())
        }
    };
    let tx_range_end = *tx_range.end();

    let mut limiter = input.limiter;

    let mut last_pruned_transaction = tx_range_end;
    let (pruned, done) = provider.tx_ref().prune_table_with_range::<tables::Receipts>(
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

    Ok(SegmentOutput {
        progress,
        pruned,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: last_pruned_block,
            tx_number: Some(last_pruned_transaction),
        }),
    })
}

pub(crate) fn save_checkpoint(
    provider: impl PruneCheckpointWriter,
    checkpoint: PruneCheckpoint,
) -> ProviderResult<()> {
    provider.save_prune_checkpoint(PruneSegment::Receipts, checkpoint)?;

    // `PruneSegment::Receipts` overrides `PruneSegment::ContractLogs`, so we can preemptively
    // limit their pruning start point.
    provider.save_prune_checkpoint(PruneSegment::ContractLogs, checkpoint)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, SegmentOutput};
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
    use reth_testing_utils::generators::{
        self, random_block_range, random_receipt, BlockRangeParams,
    };
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

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body.transactions {
                receipts
                    .push((receipts.len() as u64, random_receipt(&mut rng, transaction, Some(0))));
            }
        }
        db.insert_receipts(receipts.clone()).expect("insert receipts");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.transactions.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::Receipts>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
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
                .map(|block| block.body.transactions.len())
                .sum::<usize>()
                .min(
                    next_tx_number_to_prune as usize +
                        input.limiter.deleted_entries_limit().unwrap(),
                )
                .sub(1);

            let provider = db.factory.database_provider_rw().unwrap();
            let result = super::prune(&provider, input).unwrap();
            limiter.increment_deleted_entries_count_by(result.pruned);

            assert_matches!(
                result,
                SegmentOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            super::save_checkpoint(
                &provider,
                result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
            )
            .unwrap();
            provider.commit().expect("commit");

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
