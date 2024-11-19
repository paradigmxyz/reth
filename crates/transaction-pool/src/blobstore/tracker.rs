//! Support for maintaining the blob pool.

use alloy_primitives::{BlockNumber, B256};
use reth_execution_types::ChainBlocks;
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
    ///
    /// Replaces any previously tracked blocks with the set of transactions.
    pub fn add_blocks(
        &mut self,
        blocks: impl IntoIterator<Item = (BlockNumber, impl IntoIterator<Item = B256>)>,
    ) {
        for (block_number, blob_txs) in blocks {
            self.add_block(block_number, blob_txs);
        }
    }

    /// Adds all blob transactions from the given chain to the tracker.
    ///
    /// Note: In case this is a chain that's part of a reorg, this replaces previously tracked
    /// blocks.
    pub fn add_new_chain_blocks(&mut self, blocks: &ChainBlocks<'_>) {
        let blob_txs = blocks.iter().map(|(num, block)| {
            let iter = block
                .body
                .transactions()
                .filter(|tx| tx.transaction.is_eip4844())
                .map(|tx| tx.hash);
            (*num, iter)
        });
        self.add_blocks(blob_txs);
    }

    /// Invoked when a block is finalized.
    ///
    /// This returns all blob transactions that were included in blocks that are now finalized.
    pub fn on_finalized_block(&mut self, finalized_block: BlockNumber) -> BlobStoreUpdates {
        let mut finalized = Vec::new();
        while let Some(entry) = self.blob_txs_in_blocks.first_entry() {
            if *entry.key() <= finalized_block {
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
    use alloy_consensus::Header;
    use reth_execution_types::Chain;
    use reth_primitives::{
        BlockBody, SealedBlock, SealedBlockWithSenders, SealedHeader, Transaction,
        TransactionSigned,
    };

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

    #[test]
    fn test_add_new_chain_blocks() {
        let mut tracker = BlobStoreCanonTracker::default();

        // Create sample transactions
        let tx1_hash = B256::random(); // EIP-4844 transaction
        let tx2_hash = B256::random(); // EIP-4844 transaction
        let tx3_hash = B256::random(); // Non-EIP-4844 transaction

        // Creating a first block with EIP-4844 transactions
        let block1 = SealedBlockWithSenders {
            block: SealedBlock {
                header: SealedHeader::new(
                    Header { number: 10, ..Default::default() },
                    B256::random(),
                ),
                body: BlockBody {
                    transactions: vec![
                        TransactionSigned {
                            hash: tx1_hash,
                            transaction: Transaction::Eip4844(Default::default()),
                            ..Default::default()
                        },
                        TransactionSigned {
                            hash: tx2_hash,
                            transaction: Transaction::Eip4844(Default::default()),
                            ..Default::default()
                        },
                        // Another transaction that is not EIP-4844
                        TransactionSigned {
                            hash: B256::random(),
                            transaction: Transaction::Eip7702(Default::default()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            },
            ..Default::default()
        };

        // Creating a second block with EIP-1559 and EIP-2930 transactions
        // Note: This block does not contain any EIP-4844 transactions
        let block2 = SealedBlockWithSenders {
            block: SealedBlock {
                header: SealedHeader::new(
                    Header { number: 11, ..Default::default() },
                    B256::random(),
                ),
                body: BlockBody {
                    transactions: vec![
                        TransactionSigned {
                            hash: tx3_hash,
                            transaction: Transaction::Eip1559(Default::default()),
                            ..Default::default()
                        },
                        TransactionSigned {
                            hash: tx2_hash,
                            transaction: Transaction::Eip2930(Default::default()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            },
            ..Default::default()
        };

        // Extract blocks from the chain
        let chain: Chain = Chain::new(vec![block1, block2], Default::default(), None);
        let blocks = chain.into_inner().0;

        // Add new chain blocks to the tracker
        tracker.add_new_chain_blocks(&blocks);

        // Tx1 and tx2 should be in the block containing EIP-4844 transactions
        assert_eq!(tracker.blob_txs_in_blocks.get(&10).unwrap(), &vec![tx1_hash, tx2_hash]);
        // No transactions should be in the block containing non-EIP-4844 transactions
        assert!(tracker.blob_txs_in_blocks.get(&11).unwrap().is_empty());
    }
}
