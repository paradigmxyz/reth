use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{database::Database, tables};
use reth_primitives::PruneSegment;
use reth_provider::{DatabaseProviderRW, TransactionsProvider};
use tracing::{instrument, trace};

#[derive(Default)]
#[non_exhaustive]
pub(crate) struct Transactions;

impl Segment for Transactions {
    const SEGMENT: PruneSegment = PruneSegment::Transactions;

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        let tx_range = match input.get_next_tx_num_range_from_checkpoint(provider, Self::SEGMENT)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transactions to prune");
                return Ok(PruneOutput::done())
            }
        };

        let mut last_pruned_transaction = *tx_range.end();
        let (pruned, done) = provider.prune_table_with_range::<tables::Transactions>(
            tx_range,
            input.delete_limit,
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
    use crate::segments::{PruneInput, PruneOutput, Segment, Transactions};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db::tables;
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, TxNumber, B256};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestTransaction;
    use std::ops::Sub;

    #[test]
    fn prune() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 1..=100, B256::ZERO, 2..3);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let transactions = blocks.iter().flat_map(|block| &block.body).collect::<Vec<_>>();

        assert_eq!(tx.table::<tables::Transactions>().unwrap().len(), transactions.len());

        let test_prune = |to_block: BlockNumber, expected_result: (bool, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let input = PruneInput { to_block, delete_limit: 10 };
            let segment = Transactions::default();

            let next_tx_number_to_prune = tx
                .inner()
                .get_prune_checkpoint(Transactions::SEGMENT)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let provider = tx.inner_rw();
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
                .0
                .checked_sub(if result.done { 0 } else { 1 });

            assert_eq!(
                tx.table::<tables::Transactions>().unwrap().len(),
                transactions.len() - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                tx.inner().get_prune_checkpoint(Transactions::SEGMENT).unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(6, (false, 10));
        test_prune(6, (true, 2));
    }
}
