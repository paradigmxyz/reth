//! Helper for handling execution of multiple blocks.

use crate::{precompile::Address, primitives::alloy_primitives::BlockNumber};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{
    PruneMode, PruneModes, PruneSegmentError, Receipt, Receipts, MINIMUM_PRUNING_DISTANCE,
};
use revm::db::states::bundle_state::BundleRetention;
use std::time::Duration;
use tracing::debug;

/// Takes care of:
///  - recording receipts during execution of multiple blocks.
///  - pruning receipts according to the pruning configuration.
///  - batch range if known
#[derive(Debug, Default)]
pub struct BlockBatchRecord {
    /// Pruning configuration.
    prune_modes: PruneModes,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    receipts: Receipts,
    /// Memoized address pruning filter.
    /// Empty implies that there is going to be addresses to include in the filter in a future
    /// block. None means there isn't any kind of configuration.
    pruning_address_filter: Option<(u64, Vec<Address>)>,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    first_block: Option<BlockNumber>,
    /// The maximum known block.
    tip: Option<BlockNumber>,
}

impl BlockBatchRecord {
    /// Create a new receipts recorder with the given pruning configuration.
    pub fn new(prune_modes: PruneModes) -> Self {
        Self { prune_modes, ..Default::default() }
    }

    /// Set prune modes.
    pub fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.prune_modes = prune_modes;
    }

    /// Set the first block number of the batch.
    pub fn set_first_block(&mut self, first_block: BlockNumber) {
        self.first_block = Some(first_block);
    }

    /// Returns the first block of the batch if known.
    pub const fn first_block(&self) -> Option<BlockNumber> {
        self.first_block
    }

    /// Set tip - highest known block number.
    pub fn set_tip(&mut self, tip: BlockNumber) {
        self.tip = Some(tip);
    }

    /// Returns the tip of the batch if known.
    pub const fn tip(&self) -> Option<BlockNumber> {
        self.tip
    }

    /// Returns the recorded receipts.
    pub fn receipts(&self) -> &Receipts {
        &self.receipts
    }

    /// Returns all recorded receipts.
    pub fn take_receipts(&mut self) -> Receipts {
        std::mem::take(&mut self.receipts)
    }

    /// Returns the [BundleRetention] for the given block based on the configured prune modes.
    pub fn bundle_retention(&self, block_number: BlockNumber) -> BundleRetention {
        if self.tip.map_or(true, |tip| {
            !self
                .prune_modes
                .account_history
                .map_or(false, |mode| mode.should_prune(block_number, tip)) &&
                !self
                    .prune_modes
                    .storage_history
                    .map_or(false, |mode| mode.should_prune(block_number, tip))
        }) {
            BundleRetention::Reverts
        } else {
            BundleRetention::PlainState
        }
    }

    /// Save receipts to the executor.
    pub fn save_receipts(&mut self, receipts: Vec<Receipt>) -> Result<(), BlockExecutionError> {
        let mut receipts = receipts.into_iter().map(Some).collect();
        // Prune receipts if necessary.
        self.prune_receipts(&mut receipts)?;
        // Save receipts.
        self.receipts.push(receipts);
        Ok(())
    }

    /// Prune receipts according to the pruning configuration.
    fn prune_receipts(
        &mut self,
        receipts: &mut Vec<Option<Receipt>>,
    ) -> Result<(), PruneSegmentError> {
        let (first_block, tip) = match self.first_block.zip(self.tip) {
            Some((block, tip)) => (block, tip),
            _ => return Ok(()),
        };

        let block_number = first_block + self.receipts.len() as u64;

        // Block receipts should not be retained
        if self.prune_modes.receipts == Some(PruneMode::Full) ||
            // [`PruneSegment::Receipts`] takes priority over [`PruneSegment::ContractLogs`]
            self.prune_modes.receipts.map_or(false, |mode| mode.should_prune(block_number, tip))
        {
            receipts.clear();
            return Ok(())
        }

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PruneSegment::ContractLogs`].
        let prunable_receipts =
            PruneMode::Distance(MINIMUM_PRUNING_DISTANCE).should_prune(block_number, tip);
        if !prunable_receipts {
            return Ok(())
        }

        let contract_log_pruner = self.prune_modes.receipts_log_filter.group_by_block(tip, None)?;

        if !contract_log_pruner.is_empty() {
            let (prev_block, filter) = self.pruning_address_filter.get_or_insert((0, Vec::new()));
            for (_, addresses) in contract_log_pruner.range(*prev_block..=block_number) {
                filter.extend(addresses.iter().copied());
            }
        }

        for receipt in receipts.iter_mut() {
            let inner_receipt = receipt.as_ref().expect("receipts have not been pruned");

            // If there is an address_filter, and it does not contain any of the
            // contract addresses, then remove this receipts
            if let Some((_, filter)) = &self.pruning_address_filter {
                if !inner_receipt.logs.iter().any(|log| filter.contains(&log.address)) {
                    receipt.take();
                }
            }
        }

        Ok(())
    }
}

/// Block execution statistics. Contains duration of each step of block execution.
#[derive(Clone, Debug, Default)]
pub struct BlockExecutorStats {
    /// Execution duration.
    pub execution_duration: Duration,
    /// Time needed to apply output of revm execution to revm cached state.
    pub apply_state_duration: Duration,
    /// Time needed to apply post execution state changes.
    pub apply_post_execution_state_changes_duration: Duration,
    /// Time needed to merge transitions and create reverts.
    /// It this time transitions are applies to revm bundle state.
    pub merge_transitions_duration: Duration,
    /// Time needed to calculate receipt roots.
    pub receipt_root_duration: Duration,
}

impl BlockExecutorStats {
    /// Log duration to debug level log.
    pub fn log_debug(&self) {
        debug!(
            target: "evm",
            evm_transact = ?self.execution_duration,
            apply_state = ?self.apply_state_duration,
            apply_post_state = ?self.apply_post_execution_state_changes_duration,
            merge_transitions = ?self.merge_transitions_duration,
            receipt_root = ?self.receipt_root_duration,
            "Execution time"
        );
    }
}
