use reth_primitives::{
    Address, BlockNumber, ChainSpec, Hardfork, PruneMode, PruneModes, PruneSegmentError, Receipt,
    Receipts, MINIMUM_PRUNING_DISTANCE,
};
use revm::db::states::bundle_state::BundleRetention;
use std::sync::Arc;

/// Execution data required for aggregating and pruning executed block results.
#[derive(Debug)]
pub struct ExecutionData {
    /// The configured chain-spec
    pub chain_spec: Arc<ChainSpec>,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    pub first_block: Option<BlockNumber>,
    /// The maximum known block.
    pub tip: Option<BlockNumber>,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    pub receipts: Receipts,
    /// Pruning configuration.
    pub prune_modes: PruneModes,
    /// Memoized address pruning filter.
    /// Empty implies that there is going to be addresses to include in the filter in a future
    /// block. None means there isn't any kind of configuration.
    pub pruning_address_filter: Option<(u64, Vec<Address>)>,
}

impl ExecutionData {
    /// Create new [ExecutionData] from [ChainSpec].
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            chain_spec,
            receipts: Receipts::new(),
            first_block: None,
            tip: None,
            prune_modes: PruneModes::none(),
            pruning_address_filter: None,
        }
    }

    /// Returns `true` if state clear is enabled for this block.
    pub fn state_clear_enabled(&self, block: BlockNumber) -> bool {
        self.chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(block)
    }

    /// Returns the bundle retention level to be used for merging bundle state for this block.
    pub fn retention_for_block(&self, block: BlockNumber) -> BundleRetention {
        if self.tip.map_or(true, |tip| {
            !self.prune_modes.account_history.map_or(false, |mode| mode.should_prune(block, tip)) &&
                !self
                    .prune_modes
                    .storage_history
                    .map_or(false, |mode| mode.should_prune(block, tip))
        }) {
            BundleRetention::Reverts
        } else {
            BundleRetention::PlainState
        }
    }

    /// Prunes receipts according to the pruning configuration.
    pub fn prune_receipts(
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
