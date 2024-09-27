//! Helper for handling execution of multiple blocks.

use alloc::vec::Vec;
use alloy_primitives::{map::HashSet, Address, BlockNumber};
use reth_execution_errors::{BlockExecutionError, InternalBlockExecutionError};
use reth_primitives::{Receipt, Receipts, Request, Requests};
use reth_prune_types::{PruneMode, PruneModes, PruneSegmentError, MINIMUM_PRUNING_DISTANCE};
use revm::db::states::bundle_state::BundleRetention;

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
    /// The collection of EIP-7685 requests.
    /// Outer vector stores requests for each block sequentially.
    /// The inner vector stores requests ordered by transaction number.
    ///
    /// A transaction may have zero or more requests, so the length of the inner vector is not
    /// guaranteed to be the same as the number of transactions.
    requests: Vec<Requests>,
    /// Memoized address pruning filter.
    ///
    /// Empty implies that there is going to be addresses to include in the filter in a future
    /// block. None means there isn't any kind of configuration.
    pruning_address_filter: Option<(u64, HashSet<Address>)>,
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
    pub const fn receipts(&self) -> &Receipts {
        &self.receipts
    }

    /// Returns all recorded receipts.
    pub fn take_receipts(&mut self) -> Receipts {
        core::mem::take(&mut self.receipts)
    }

    /// Returns the recorded requests.
    pub fn requests(&self) -> &[Requests] {
        &self.requests
    }

    /// Returns all recorded requests.
    pub fn take_requests(&mut self) -> Vec<Requests> {
        core::mem::take(&mut self.requests)
    }

    /// Returns the [`BundleRetention`] for the given block based on the configured prune modes.
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
        self.prune_receipts(&mut receipts).map_err(InternalBlockExecutionError::from)?;
        // Save receipts.
        self.receipts.push(receipts);
        Ok(())
    }

    /// Prune receipts according to the pruning configuration.
    fn prune_receipts(
        &mut self,
        receipts: &mut Vec<Option<Receipt>>,
    ) -> Result<(), PruneSegmentError> {
        let (Some(first_block), Some(tip)) = (self.first_block, self.tip) else { return Ok(()) };

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
            let (prev_block, filter) =
                self.pruning_address_filter.get_or_insert_with(|| (0, Default::default()));
            for (_, addresses) in contract_log_pruner.range(*prev_block..=block_number) {
                filter.extend(addresses.iter().copied());
            }
        }

        if let Some((_, filter)) = &self.pruning_address_filter {
            for receipt in receipts.iter_mut() {
                // If there is an address_filter, it does not contain any of the
                // contract addresses, then remove this receipt.
                let inner_receipt = receipt.as_ref().expect("receipts have not been pruned");
                if !inner_receipt.logs.iter().any(|log| filter.contains(&log.address)) {
                    receipt.take();
                }
            }
        }

        Ok(())
    }

    /// Save EIP-7685 requests to the executor.
    pub fn save_requests(&mut self, requests: Vec<Request>) {
        self.requests.push(requests.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloy_primitives::Address;
    use reth_primitives::{Log, Receipt};
    use reth_prune_types::{PruneMode, ReceiptsLogPruneConfig};

    #[test]
    fn test_save_receipts_empty() {
        let mut recorder = BlockBatchRecord::default();
        // Create an empty vector of receipts
        let receipts = vec![];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that the saved receipts are equal to a nested empty vector
        assert_eq!(*recorder.receipts(), vec![vec![]].into());
    }

    #[test]
    fn test_save_receipts_non_empty_no_pruning() {
        let mut recorder = BlockBatchRecord::default();
        let receipts = vec![Receipt::default()];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that there is one block of receipts
        assert_eq!(recorder.receipts().len(), 1);
        // Verify that the first block contains one receipt
        assert_eq!(recorder.receipts()[0].len(), 1);
        // Verify that the saved receipt is the default receipt
        assert_eq!(recorder.receipts()[0][0], Some(Receipt::default()));
    }

    #[test]
    fn test_save_receipts_with_pruning_no_prunable_receipts() {
        let mut recorder = BlockBatchRecord::default();

        // Set the first block number
        recorder.set_first_block(1);
        // Set the tip (highest known block)
        recorder.set_tip(130);

        // Create a vector of receipts with a default receipt
        let receipts = vec![Receipt::default()];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that there is one block of receipts
        assert_eq!(recorder.receipts().len(), 1);
        // Verify that the first block contains one receipt
        assert_eq!(recorder.receipts()[0].len(), 1);
        // Verify that the saved receipt is the default receipt
        assert_eq!(recorder.receipts()[0][0], Some(Receipt::default()));
    }

    #[test]
    fn test_save_receipts_with_pruning_no_tip() {
        // Create a PruneModes with receipts set to PruneMode::Full
        let prune_modes = PruneModes { receipts: Some(PruneMode::Full), ..Default::default() };

        let mut recorder = BlockBatchRecord::new(prune_modes);

        // Set the first block number
        recorder.set_first_block(1);
        // Create a vector of receipts with a default receipt
        let receipts = vec![Receipt::default()];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that there is one block of receipts
        assert_eq!(recorder.receipts().len(), 1);
        // Verify that the first block contains one receipt
        assert_eq!(recorder.receipts()[0].len(), 1);
        // Verify that the saved receipt is the default receipt
        assert_eq!(recorder.receipts()[0][0], Some(Receipt::default()));
    }

    #[test]
    fn test_save_receipts_with_pruning_no_block_number() {
        // Create a PruneModes with receipts set to PruneMode::Full
        let prune_modes = PruneModes { receipts: Some(PruneMode::Full), ..Default::default() };

        // Create a BlockBatchRecord with the prune_modes
        let mut recorder = BlockBatchRecord::new(prune_modes);

        // Set the tip (highest known block)
        recorder.set_tip(130);

        // Create a vector of receipts with a default receipt
        let receipts = vec![Receipt::default()];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that there is one block of receipts
        assert_eq!(recorder.receipts().len(), 1);
        // Verify that the first block contains one receipt
        assert_eq!(recorder.receipts()[0].len(), 1);
        // Verify that the saved receipt is the default receipt
        assert_eq!(recorder.receipts()[0][0], Some(Receipt::default()));
    }

    // Test saving receipts with pruning configuration and receipts should be pruned
    #[test]
    fn test_save_receipts_with_pruning_should_prune() {
        // Create a PruneModes with receipts set to PruneMode::Full
        let prune_modes = PruneModes { receipts: Some(PruneMode::Full), ..Default::default() };

        // Create a BlockBatchRecord with the prune_modes
        let mut recorder = BlockBatchRecord::new(prune_modes);

        // Set the first block number
        recorder.set_first_block(1);
        // Set the tip (highest known block)
        recorder.set_tip(130);

        // Create a vector of receipts with a default receipt
        let receipts = vec![Receipt::default()];

        // Verify that saving receipts completes without error
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that there is one block of receipts
        assert_eq!(recorder.receipts().len(), 1);
        // Verify that the receipts are pruned (empty)
        assert!(recorder.receipts()[0].is_empty());
    }

    // Test saving receipts with address filter pruning
    #[test]
    fn test_save_receipts_with_address_filter_pruning() {
        // Create a PruneModes with receipts_log_filter configuration
        let prune_modes = PruneModes {
            receipts_log_filter: ReceiptsLogPruneConfig(BTreeMap::from([
                (Address::with_last_byte(1), PruneMode::Before(1300001)),
                (Address::with_last_byte(2), PruneMode::Before(1300002)),
                (Address::with_last_byte(3), PruneMode::Distance(1300003)),
            ])),
            ..Default::default()
        };

        // Create a BlockBatchRecord with the prune_modes
        let mut recorder = BlockBatchRecord::new(prune_modes);

        // Set the first block number
        recorder.set_first_block(1);
        // Set the tip (highest known block)
        recorder.set_tip(1300000);

        // With a receipt that should be pruned (address 4 not in the log filter)
        let mut receipt = Receipt::default();
        receipt.logs.push(Log { address: Address::with_last_byte(4), ..Default::default() });
        let receipts = vec![receipt.clone()];
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that the receipts are pruned (empty)
        assert_eq!(recorder.receipts().len(), 1);
        assert_eq!(recorder.receipts()[0], vec![None]);

        // With a receipt that should not be pruned (address 1 in the log filter)
        let mut receipt1 = Receipt::default();
        receipt1.logs.push(Log { address: Address::with_last_byte(1), ..Default::default() });
        let receipts = vec![receipt1.clone()];
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that the second block of receipts contains the receipt
        assert_eq!(recorder.receipts().len(), 2);
        assert_eq!(recorder.receipts()[1][0], Some(receipt1));

        // With a receipt that should not be pruned (address 2 in the log filter)
        let mut receipt2 = Receipt::default();
        receipt2.logs.push(Log { address: Address::with_last_byte(2), ..Default::default() });
        let receipts = vec![receipt2.clone()];
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that the third block of receipts contains the receipt
        assert_eq!(recorder.receipts().len(), 3);
        assert_eq!(recorder.receipts()[2][0], Some(receipt2));

        // With a receipt that should not be pruned (address 3 in the log filter)
        let mut receipt3 = Receipt::default();
        receipt3.logs.push(Log { address: Address::with_last_byte(3), ..Default::default() });
        let receipts = vec![receipt3.clone()];
        assert!(recorder.save_receipts(receipts).is_ok());
        // Verify that the fourth block of receipts contains the receipt
        assert_eq!(recorder.receipts().len(), 4);
        assert_eq!(recorder.receipts()[3][0], Some(receipt3));
    }
}
