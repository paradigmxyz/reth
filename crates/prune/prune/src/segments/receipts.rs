//! Common receipts pruning logic.
//!
//! - [`crate::segments::user::Receipts`] is responsible for pruning receipts according to the
//!   user-configured settings (for example, on a full node or with a custom prune config)

use crate::{
    db_ext::DbTxPruneExt,
    segments::{self, PruneInput},
    PrunerError,
};
use reth_db_api::{table::Value, tables, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DBProvider, EitherWriter,
    NodePrimitivesProvider, PruneCheckpointWriter, StaticFileProviderFactory, StorageSettingsCache,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment, SegmentOutput, SegmentOutputCheckpoint};
use reth_static_file_types::StaticFileSegment;
use tracing::{debug, trace};

pub(crate) fn prune<Provider>(
    provider: &Provider,
    input: PruneInput,
) -> Result<SegmentOutput, PrunerError>
where
    Provider: DBProvider<Tx: DbTxMut>
        + BlockReader
        + StorageSettingsCache
        + StaticFileProviderFactory
        + NodePrimitivesProvider<Primitives: NodePrimitives<Receipt: Value>>,
{
    if EitherWriter::receipts_destination(provider).is_static_file() {
        debug!(target: "pruner", "Pruning receipts from static files.");
        return segments::prune_static_files(provider, input, StaticFileSegment::Receipts)
    }
    debug!(target: "pruner", "Pruning receipts from database.");

    // Original database implementation for when receipts are not on static files (old nodes)
    let tx_range = match input.get_next_tx_num_range(provider)? {
        Some(range) => range,
        None => {
            trace!(target: "pruner", "No receipts to prune");
            return Ok(SegmentOutput::done())
        }
    };
    let tx_range_end = *tx_range.end();

    let mut limiter = input.limiter;

    let mut last_pruned_transaction = None;
    let (pruned, done) = provider.tx_ref().prune_table_with_range::<tables::Receipts<
        <Provider::Primitives as NodePrimitives>::Receipt,
    >>(
        tx_range,
        &mut limiter,
        |_| false,
        |row| last_pruned_transaction = Some(row.0),
    )?;
    trace!(target: "pruner", %pruned, %done, "Pruned receipts");

    // A limit reached before the first row means nothing in the range was pruned. Returning no
    // checkpoint keeps the stored ones, including `ContractLogs`, which `save_checkpoint` writes.
    if last_pruned_transaction.is_none() && !done {
        return Ok(SegmentOutput::not_done(limiter.interrupt_reason(), None))
    }
    let last_pruned_transaction = last_pruned_transaction.unwrap_or(tx_range_end);

    let last_pruned_block = provider
        .block_by_transaction_id(last_pruned_transaction)?
        .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
        // If there's more receipts to prune, set the checkpoint block number to previous,
        // so we could finish pruning its receipts on the next run.
        .checked_sub(if done { 0 } else { 1 });

    let progress = limiter.progress(done);

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
    use crate::{
        segments::{PruneInput, PruneLimiter, Segment, SegmentOutput, UserReceipts},
        Pruner, PrunerError,
    };
    use alloy_primitives::{BlockNumber, TxNumber, B256};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db_api::tables;
    use reth_exex_types::FinishedExExHeight;
    use reth_provider::{
        errors::provider::ProviderResult, DBProvider, DatabaseProviderFactory,
        PruneCheckpointReader, PruneCheckpointWriter,
    };
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneMode, PruneProgress, PrunePurpose, PruneSegment,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{
        self, random_block_range, random_receipt, BlockRangeParams,
    };
    use std::{ops::Sub, thread::sleep, time::Duration};
    use tokio::sync::watch;

    /// [`UserReceipts`] whose time limit expires right after the pruner's own limit check, so the
    /// segment sees the timeout before it deletes anything.
    #[derive(Debug)]
    struct ExpiredTimeLimit(UserReceipts);

    impl<Provider> Segment<Provider> for ExpiredTimeLimit
    where
        UserReceipts: Segment<Provider>,
    {
        fn segment(&self) -> PruneSegment {
            self.0.segment()
        }

        fn mode(&self) -> Option<PruneMode> {
            self.0.mode()
        }

        fn purpose(&self) -> PrunePurpose {
            self.0.purpose()
        }

        fn prune(
            &self,
            provider: &Provider,
            mut input: PruneInput,
        ) -> Result<SegmentOutput, PrunerError> {
            input.limiter = input.limiter.set_time_limit(Duration::ZERO);
            sleep(Duration::from_millis(1));
            self.0.prune(provider, input)
        }

        fn save_checkpoint(
            &self,
            provider: &Provider,
            checkpoint: PruneCheckpoint,
        ) -> ProviderResult<()>
        where
            Provider: PruneCheckpointWriter,
        {
            self.0.save_checkpoint(provider, checkpoint)
        }
    }

    #[test]
    fn prune_legacy() {
        let mut db = TestStageDB::default();
        // Configure the factory to use database for receipts by enabling receipt pruning.
        // This ensures EitherWriter::receipts_destination returns Database instead of StaticFile.
        db.factory = db.factory.with_prune_modes(reth_prune_types::PruneModes {
            receipts: Some(PruneMode::Full),
            ..Default::default()
        });
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut receipts = Vec::new();
        for block in &blocks {
            receipts.reserve_exact(block.transaction_count());
            for transaction in &block.body().transactions {
                receipts.push((
                    receipts.len() as u64,
                    random_receipt(&mut rng, transaction, Some(0), None),
                ));
            }
        }
        let receipts_len = receipts.len();
        db.insert_receipts(receipts).expect("insert receipts");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.transaction_count()).sum::<usize>()
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
                .map(|block| block.transaction_count())
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
                    tx_count += block.transaction_count();

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
                receipts_len - (last_pruned_tx_number + 1)
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

        // A timeout before the first delete keeps every stored checkpoint. `save_checkpoint` also
        // writes `ContractLogs`, so this goes through `Pruner::run`.
        let contract_logs = PruneCheckpoint {
            block_number: Some(5),
            tx_number: Some(9),
            prune_mode: PruneMode::Before(7),
        };
        let provider = db.factory.database_provider_rw().unwrap();
        provider.save_prune_checkpoint(PruneSegment::ContractLogs, contract_logs).unwrap();
        provider.commit().expect("commit");
        let receipts_checkpoint =
            db.factory.provider().unwrap().get_prune_checkpoint(PruneSegment::Receipts).unwrap();
        let rows = db.table::<tables::Receipts>().unwrap().len();
        let (_, finished_height_rx) = watch::channel(FinishedExExHeight::NoExExs);
        let mut pruner = Pruner::new_with_factory(
            db.factory.clone(),
            vec![Box::new(ExpiredTimeLimit(UserReceipts::new(PruneMode::Before(7))))],
            1,
            usize::MAX,
            None,
            finished_height_rx,
        )
        .with_minimum_pruning_distance(0);
        let output = pruner.run(10).unwrap();
        assert_eq!(output.progress, PruneProgress::HasMoreData(PruneInterruptReason::Timeout));
        assert_eq!(db.table::<tables::Receipts>().unwrap().len(), rows);
        let provider = db.factory.provider().unwrap();
        assert_eq!(
            provider.get_prune_checkpoint(PruneSegment::Receipts).unwrap(),
            receipts_checkpoint
        );
        assert_eq!(
            provider.get_prune_checkpoint(PruneSegment::ContractLogs).unwrap(),
            Some(contract_logs)
        );

        test_prune(6, (PruneProgress::Finished, 2));
        test_prune(10, (PruneProgress::Finished, 8));
    }
}
