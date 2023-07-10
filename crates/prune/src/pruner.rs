//! Support for pruning.

use crate::PrunerError;
use reth_db::{database::Database, tables};
use reth_primitives::{BlockNumber, ChainSpec};
use reth_provider::{
    BlockReader, DatabaseProvider, DatabaseProviderRW, ProviderError, ProviderFactory,
    TransactionsProvider,
};
use std::sync::Arc;
use tracing::debug;

/// Result of [Pruner::run] execution
pub type PrunerResult = Result<(), PrunerError>;

/// The pipeline type itself with the result of [Pruner::run]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

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
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        min_block_interval: u64,
        max_prune_depth: u64,
    ) -> Self {
        Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            min_block_interval,
            max_prune_depth,
            last_pruned_block_number: None,
        }
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        let provider = self.provider_factory.provider_rw()?;

        self.prune_receipts(&provider, tip_block_number)?;

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

    fn prune_receipts(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        tip_block_number: BlockNumber,
    ) -> PrunerResult {
        const PRUNE_RECEIPTS: u64 = 64;
        let to_block = tip_block_number - PRUNE_RECEIPTS;
        let body = provider
            .block_body_indices(to_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(to_block))?;

        debug!(target: "pruner", %to_block, "Pruning receipts");
        let pruned_receipts = provider.prune_table::<tables::Receipts, _>(..=body.last_tx_num())?;
        debug!(target: "pruner", %to_block, pruned = %pruned_receipts, "Finished pruning receipts");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;

    #[test]
    fn pruner_is_pruning_needed() {
        let pruner = Pruner::new(5, 0);

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
}
