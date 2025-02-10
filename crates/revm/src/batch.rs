//! Helper for handling execution of multiple blocks.

use alloc::vec::Vec;

use alloy_eips::eip7685::Requests;
use alloy_primitives::BlockNumber;

/// Takes care of:
///  - recording receipts during execution of multiple blocks.
///  - pruning receipts according to the pruning configuration.
///  - batch range if known
#[derive(Debug)]
pub struct BlockBatchRecord<T = reth_ethereum_primitives::Receipt> {
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    receipts: Vec<Vec<T>>,
    /// The collection of EIP-7685 requests.
    /// Outer vector stores requests for each block sequentially.
    /// The inner vector stores requests ordered by transaction number.
    ///
    /// A transaction may have zero or more requests, so the length of the inner vector is not
    /// guaranteed to be the same as the number of transactions.
    requests: Vec<Requests>,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    first_block: Option<BlockNumber>,
}

impl<T> Default for BlockBatchRecord<T> {
    fn default() -> Self {
        Self {
            receipts: Default::default(),
            requests: Default::default(),
            first_block: Default::default(),
        }
    }
}

impl<T> BlockBatchRecord<T> {
    /// Create a new receipts recorder with the given pruning configuration.
    pub fn new() -> Self {
        Default::default()
    }

    /// Set the first block number of the batch.
    pub fn set_first_block(&mut self, first_block: BlockNumber) {
        self.first_block = Some(first_block);
    }

    /// Returns the first block of the batch if known.
    pub const fn first_block(&self) -> Option<BlockNumber> {
        self.first_block
    }

    /// Returns the recorded receipts.
    pub const fn receipts(&self) -> &Vec<Vec<T>> {
        &self.receipts
    }

    /// Returns all recorded receipts.
    pub fn take_receipts(&mut self) -> Vec<Vec<T>> {
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

    /// Save receipts to the executor.
    pub fn save_receipts(&mut self, receipts: Vec<T>) {
        self.receipts.push(receipts);
    }

    /// Save EIP-7685 requests to the executor.
    pub fn save_requests(&mut self, requests: Requests) {
        self.requests.push(requests);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_receipts_empty() {
        let mut recorder: BlockBatchRecord = BlockBatchRecord::default();
        // Create an empty vector of receipts
        let receipts = vec![];

        recorder.save_receipts(receipts);
        // Verify that the saved receipts are equal to a nested empty vector
        assert_eq!(*recorder.receipts(), vec![vec![]]);
    }
}
