//! Support for pruning.

use crate::PrunerError;
use reth_db::{database::Database, tables};
use reth_primitives::{BlockNumber, ChainSpec, PruneCheckpoint, PruneMode, PruneModes, PrunePart};
use reth_provider::{BlockReader, DatabaseProviderRW, ProviderFactory, PruneCheckpointWriter};
use std::sync::Arc;
use tracing::{debug, instrument, trace};

/// Result of [Pruner::run] execution
pub type PrunerResult = Result<(), PrunerError>;

/// The pipeline type itself with the result of [Pruner::run]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

pub struct BatchSizes {
    receipts: usize,
}

impl Default for BatchSizes {
    fn default() -> Self {
        Self { receipts: 10000 }
    }
}

/// Pruning routine. Main pruning logic happens in [Pruner::run].
pub struct Pruner<DB> {
    provider_factory: ProviderFactory<DB>,
    /// Minimum pruning interval measured in blocks. All prune parts are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: u64,
    /// Maximum prune depth. Used to determine the pruning target for parts that are needed during
    /// the reorg, e.g. changesets.
    #[allow(dead_code)]
    max_prune_depth: u64,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
    modes: PruneModes,
    batch_sizes: BatchSizes,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        min_block_interval: u64,
        max_prune_depth: u64,
        modes: PruneModes,
        batch_sizes: BatchSizes,
    ) -> Self {
        Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            min_block_interval,
            max_prune_depth,
            last_pruned_block_number: None,
            modes,
            batch_sizes,
        }
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        let provider = self.provider_factory.provider_rw()?;

        if let Some((to_block, prune_mode)) = self.modes.prune_to_block_receipts(tip_block_number) {
            self.prune_receipts(&provider, to_block, prune_mode)?;
        }

        provider.commit()?;

        self.last_pruned_block_number = Some(tip_block_number);
        Ok(())
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        if self.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previously pruned block number. If
            // that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(last_pruned_block_number) >= self.min_block_interval
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

    /// Prune receipts up to the provided block, inclusive.
    #[instrument(level = "trace", skip(self, provider), target = "pruner")]
    fn prune_receipts(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        to_block: BlockNumber,
        prune_mode: PruneMode,
    ) -> PrunerResult {
        let to_block_body = match provider.block_body_indices(to_block)? {
            Some(body) => body,
            None => {
                trace!(target: "pruner", "No receipts to prune");
                return Ok(())
            }
        };

        provider.prune_table_in_batches::<tables::Receipts, _, _>(
            ..=to_block_body.last_tx_num(),
            self.batch_sizes.receipts,
            |receipts| {
                trace!(
                    target: "pruner",
                    %receipts,
                    "Pruned receipts"
                );
            },
        )?;

        provider.save_prune_checkpoint(
            PrunePart::Receipts,
            PruneCheckpoint { block_number: to_block, prune_mode },
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{pruner::BatchSizes, Pruner};
    use assert_matches::assert_matches;
    use reth_db::{tables, test_utils::create_test_rw_db};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_receipt},
    };
    use reth_primitives::{PruneCheckpoint, PruneMode, PruneModes, PrunePart, H256, MAINNET};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestTransaction;

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let pruner =
            Pruner::new(db, MAINNET.clone(), 5, 0, PruneModes::default(), BatchSizes::default());

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));

        // Delta is not less than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval;
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

        let prune_to_block = 10;
        let prune_mode = PruneMode::Before(prune_to_block);
        let pruner = Pruner::new(
            tx.inner_raw(),
            MAINNET.clone(),
            5,
            0,
            PruneModes { receipts: Some(prune_mode), ..Default::default() },
            BatchSizes {
                // Less than total amount of blocks to prune to test the batching logic
                receipts: 10,
            },
        );

        let provider = tx.inner_rw();
        assert_matches!(pruner.prune_receipts(&provider, prune_to_block, prune_mode), Ok(()));
        provider.commit().expect("commit");

        assert_eq!(
            tx.table::<tables::Receipts>().unwrap().len(),
            blocks[prune_to_block as usize + 1..]
                .iter()
                .map(|block| block.body.len())
                .sum::<usize>()
        );
        assert_eq!(
            tx.inner().get_prune_checkpoint(PrunePart::Receipts).unwrap(),
            Some(PruneCheckpoint { block_number: prune_to_block, prune_mode })
        );
    }
}
