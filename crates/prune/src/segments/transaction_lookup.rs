use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use rayon::prelude::*;
use reth_db::{database::Database, tables};
use reth_primitives::{PruneMode, PruneSegment};
use reth_provider::{DatabaseProviderRW, TransactionsProvider};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct TransactionLookup {
    mode: PruneMode,
}

impl TransactionLookup {
    pub fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for TransactionLookup {
    fn segment(&self) -> PruneSegment {
        PruneSegment::TransactionLookup
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
        let (start, end) = match input.get_next_tx_num_range(provider)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction lookup entries to prune");
                return Ok(PruneOutput::done())
            }
        }
        .into_inner();
        let tx_range = start..=(end.min(start + input.delete_limit as u64 - 1));
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

        let mut last_pruned_transaction = None;
        let (pruned, _) = provider.prune_table_with_iterator::<tables::TxHashNumber>(
            hashes,
            input.delete_limit,
            |row| {
                last_pruned_transaction = Some(last_pruned_transaction.unwrap_or(row.1).max(row.1))
            },
        )?;
        let done = tx_range_end == end;
        trace!(target: "pruner", %pruned, %done, "Pruned transaction lookup");

        let last_pruned_transaction = last_pruned_transaction.unwrap_or(tx_range_end);

        let last_pruned_block = provider
            .transaction_block(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction lookup entries to prune, set the checkpoint block number
            // to previous, so we could finish pruning its transaction lookup entries on the next
            // run.
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
    use crate::segments::{PruneInput, PruneOutput, Segment, TransactionLookup};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, TxNumber, B256};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestStageDB;
    use std::ops::Sub;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 1..=10, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                tx_hash_numbers.push((transaction.hash, tx_hash_numbers.len() as u64));
            }
        }
        db.insert_tx_hash_numbers(tx_hash_numbers.clone()).expect("insert tx hash numbers");

        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            blocks.iter().map(|block| block.body.len()).sum::<usize>()
        );
        assert_eq!(
            db.table::<tables::Transactions>().unwrap().len(),
            db.table::<tables::TxHashNumber>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (bool, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::TransactionLookup)
                    .unwrap(),
                to_block,
                delete_limit: 10,
            };
            let segment = TransactionLookup::new(prune_mode);

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
                db.table::<tables::TxHashNumber>().unwrap().len(),
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

        test_prune(6, (false, 10));
        test_prune(6, (true, 2));
        test_prune(10, (true, 8));
    }
}
