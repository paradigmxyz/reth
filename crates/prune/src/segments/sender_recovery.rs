use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{database::Database, tables};
use reth_primitives::{PruneMode, PruneSegment};
use reth_provider::{DatabaseProviderRW, TransactionsProvider};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct SenderRecovery {
    mode: PruneMode,
}

impl SenderRecovery {
    pub fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for SenderRecovery {
    fn segment(&self) -> PruneSegment {
        PruneSegment::SenderRecovery
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
                trace!(target: "pruner", "No transaction senders to prune");
                return Ok(PruneOutput::done())
            }
        };
        let tx_range_end = *tx_range.end();

        let mut last_pruned_transaction = tx_range_end;
        let (pruned, done) = provider.prune_table_with_range::<tables::TransactionSenders>(
            tx_range,
            input.delete_limit,
            |_| false,
            |row| last_pruned_transaction = row.0,
        )?;
        trace!(target: "pruner", %pruned, %done, "Pruned transaction senders");

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction senders to prune, set the checkpoint block number to
            // previous, so we could finish pruning its transaction senders on the next run.
            .checked_sub(if done { 0 } else { 1 });

        Ok(PruneOutput {
            done,
            pruned,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, PruneOutput, Segment, SenderRecovery};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, TxNumber, B256};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use std::ops::Sub;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 1..=10, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut transaction_senders = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                transaction_senders.push((
                    transaction_senders.len() as u64,
                    transaction.recover_signer().expect("recover signer"),
                ));
            }
        }
        db.insert_transaction_senders(transaction_senders.clone())
            .expect("insert transaction senders");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::TransactionSenders>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (bool, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::SenderRecovery)
                    .unwrap(),
                to_block,
                delete_limit: 10,
            };
            let segment = SenderRecovery::new(prune_mode);

            let next_tx_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::SenderRecovery)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number = blocks
                .iter()
                .take(to_block as usize)
                .map(|block| block.body.len())
                .sum::<usize>()
                .min(next_tx_number_to_prune as usize + input.delete_limit)
                .sub(1);

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

            let provider = db.factory.provider_rw().unwrap();
            let result = segment.prune(&provider, input).unwrap();
            assert_matches!(
                result,
                PruneOutput {done, pruned, checkpoint: Some(_)}
                    if (done, pruned) == expected_result
            );
            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number =
                last_pruned_block_number.checked_sub(if result.done { 0 } else { 1 });

            assert_eq!(
                db.table::<tables::TransactionSenders>().unwrap().len(),
                transaction_senders.len() - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::SenderRecovery)
                    .unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(6, (false, 10));
        test_prune(6, (true, 2));
        test_prune(10, (true, 8));
    }
}
