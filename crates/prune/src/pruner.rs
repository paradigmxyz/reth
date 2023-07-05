//! Support for pruning.

use crate::PrunerError;
use reth_primitives::BlockNumber;
use std::{future::Future, pin::Pin};
use tracing::debug;

/// The future that returns the owned pipeline and the result of the pipeline run. See
/// [Pruner::run_as_fut].
pub type PrunerFut = Pin<Box<dyn Future<Output = PrunerWithResult> + Send>>;

/// The pipeline type itself with the result of [Pruner::run_as_fut]
pub type PrunerWithResult = (Pruner, Result<(), PrunerError>);

/// Pruning routine. Main pruning logic happens in [Pruner::run].
pub struct Pruner {
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

impl Pruner {
    /// Creates a new [Pruner].
    pub fn new(min_block_interval: u64, max_prune_depth: u64) -> Self {
        Self { min_block_interval, max_prune_depth, last_pruned_block_number: None }
    }

    /// Consume the pruner and run it until it finishes.
    /// Return the pruner and its result as a future.
    #[track_caller]
    pub fn run_as_fut(mut self, tip_block_number: BlockNumber) -> PrunerFut {
        Box::pin(async move {
            let result = self.run(tip_block_number).await;
            (self, result)
        })
    }

    /// Run the pruner
    pub async fn run(&mut self, tip_block_number: BlockNumber) -> Result<(), PrunerError> {
        // Pruning logic

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
}

#[cfg(test)]
mod tests {
    use crate::Pruner;

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
