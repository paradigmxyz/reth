//! Support for maintaining the blob pool.

use reth_primitives::{BlockNumber, B256};
use reth_provider::chain::ChainBlocks;
use std::collections::BTreeMap;

/// The type that is used to track canonical blob transactions.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct BlobStoreCanonTracker {
    /// Keeps track of the blob transactions included in blocks.
    blob_txs_in_blocks: BTreeMap<BlockNumber, Vec<B256>>,
}

impl BlobStoreCanonTracker {
    /// Adds a block to the blob store maintenance.
    pub fn add_block(
        &mut self,
        block_number: BlockNumber,
        blob_txs: impl IntoIterator<Item = B256>,
    ) {
        self.blob_txs_in_blocks.insert(block_number, blob_txs.into_iter().collect());
    }

    /// Adds all blocks to the tracked list of blocks.
    pub fn add_blocks(
        &mut self,
        blocks: impl IntoIterator<Item = (BlockNumber, impl IntoIterator<Item = B256>)>,
    ) {
        for (block_number, blob_txs) in blocks {
            self.add_block(block_number, blob_txs);
        }
    }

    /// Adds all blob transactions from the given chain to the tracker.
    pub fn add_new_chain_blocks(&mut self, blocks: &ChainBlocks<'_>) {
        let blob_txs = blocks.iter().map(|(num, blocks)| {
            let iter =
                blocks.body.iter().filter(|tx| tx.transaction.is_eip4844()).map(|tx| tx.hash);
            (*num, iter)
        });
        self.add_blocks(blob_txs);
    }

    /// Invoked when a block is finalized.
    pub fn on_finalized_block(&mut self, number: BlockNumber) -> BlobStoreUpdates {
        let mut finalized = Vec::new();
        while let Some(entry) = self.blob_txs_in_blocks.first_entry() {
            if *entry.key() <= number {
                finalized.extend(entry.remove_entry().1);
            } else {
                break
            }
        }

        if finalized.is_empty() {
            BlobStoreUpdates::None
        } else {
            BlobStoreUpdates::Finalized(finalized)
        }
    }
}

/// Updates that should be applied to the blob store.
#[derive(Debug, Eq, PartialEq)]
pub enum BlobStoreUpdates {
    /// No updates.
    None,
    /// Delete the given finalized transactions from the blob store.
    Finalized(Vec<B256>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finalized_tracker() {
        let mut tracker = BlobStoreCanonTracker::default();

        let block1 = vec![B256::random()];
        let block2 = vec![B256::random()];
        let block3 = vec![B256::random()];
        tracker.add_block(1, block1.clone());
        tracker.add_block(2, block2.clone());
        tracker.add_block(3, block3.clone());

        assert_eq!(tracker.on_finalized_block(0), BlobStoreUpdates::None);
        assert_eq!(tracker.on_finalized_block(1), BlobStoreUpdates::Finalized(block1));
        assert_eq!(
            tracker.on_finalized_block(3),
            BlobStoreUpdates::Finalized(block2.into_iter().chain(block3).collect::<Vec<_>>())
        );
    }
}
