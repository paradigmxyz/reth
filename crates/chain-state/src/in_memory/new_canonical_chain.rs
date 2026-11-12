use super::ExecutedBlock;
use crate::CanonStateNotification;
use alloy_primitives::B256;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SignedTransaction};
use std::sync::Arc;

/// Non-empty chain of blocks.
#[derive(Debug)]
pub enum NewCanonicalChain<N: NodePrimitives = EthPrimitives> {
    /// A simple append to the current canonical head
    Commit {
        /// all blocks that lead back to the canonical head
        new: Vec<ExecutedBlock<N>>,
    },
    /// A reorged chain consists of two chains that trace back to a shared ancestor block at which
    /// point they diverge.
    Reorg {
        /// All blocks of the _new_ chain
        new: Vec<ExecutedBlock<N>>,
        /// All blocks of the _old_ chain
        old: Vec<ExecutedBlock<N>>,
    },
}

impl<N: NodePrimitives<SignedTx: SignedTransaction>> NewCanonicalChain<N> {
    /// Returns the length of the new chain.
    pub const fn new_block_count(&self) -> usize {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.len(),
        }
    }

    /// Returns the length of the reorged chain.
    pub const fn reorged_block_count(&self) -> usize {
        match self {
            Self::Commit { .. } => 0,
            Self::Reorg { old, .. } => old.len(),
        }
    }

    /// Converts the new chain into a notification that will be emitted to listeners
    pub fn to_chain_notification(&self) -> CanonStateNotification<N> {
        match self {
            Self::Commit { new } => {
                CanonStateNotification::Commit { new: Arc::new(Self::blocks_to_chain(new)) }
            }
            Self::Reorg { new, old } => CanonStateNotification::Reorg {
                new: Arc::new(Self::blocks_to_chain(new)),
                old: Arc::new(Self::blocks_to_chain(old)),
            },
        }
    }

    /// Converts a slice of executed blocks into a [`Chain`].
    fn blocks_to_chain(blocks: &[ExecutedBlock<N>]) -> Chain<N> {
        let mut chain = match blocks {
            [] => Chain::default(),
            [first, rest @ ..] => {
                let mut chain = Chain::from_block(
                    Arc::clone(&first.recovered_block),
                    ExecutionOutcome::from((
                        first.execution_outcome().clone(),
                        first.block_number(),
                    )),
                    first.trie_data_handle(),
                );
                for exec in rest {
                    chain.append_block(
                        Arc::clone(&exec.recovered_block),
                        ExecutionOutcome::from((
                            exec.execution_outcome().clone(),
                            exec.block_number(),
                        )),
                        exec.trie_data_handle(),
                    );
                }
                chain
            }
        };
        for exec in blocks {
            if let Some(bal) = exec.bal() {
                chain.insert_bal(exec.block_number(), Arc::clone(bal));
            }
        }
        chain
    }

    /// Returns the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &RecoveredBlock<N::Block> {
        self.new_blocks().last().expect("non empty blocks").recovered_block()
    }

    /// Returns the blocks added to the canonical chain by this update.
    pub fn new_blocks(&self) -> &[ExecutedBlock<N>] {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new,
        }
    }

    /// Returns whether the newly canonicalized blocks contain the given hash.
    ///
    /// This does not include the unchanged canonical prefix before the first new block.
    pub fn contains(&self, hash: B256) -> bool {
        self.new_blocks().iter().any(|block| block.recovered_block().hash() == hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::Bytes;
    use reth_execution_types::DecodedRevmBal;
    use reth_trie::LazyTrieData;
    use std::collections::BTreeMap;

    #[test]
    fn test_to_chain_notification() {
        // Generate 4 blocks
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block1a =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block2 =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());
        let block2a =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());

        // Test commit notification
        let chain_commit = NewCanonicalChain::Commit { new: vec![block0.clone(), block1.clone()] };

        // Build expected trie data map
        let mut expected_trie_data = BTreeMap::new();
        expected_trie_data.insert(0, LazyTrieData::ready(block0.trie_data()));
        expected_trie_data.insert(1, LazyTrieData::ready(block1.trie_data()));

        // Build expected execution outcome (first_block matches first block number)
        let commit_execution_outcome = ExecutionOutcome {
            receipts: vec![vec![], vec![]],
            requests: vec![Requests::default(), Requests::default()],
            first_block: 0,
            ..Default::default()
        };

        assert_eq!(
            chain_commit.to_chain_notification(),
            CanonStateNotification::Commit {
                new: Arc::new(Chain::new(
                    vec![block0.recovered_block().clone(), block1.recovered_block().clone()],
                    commit_execution_outcome,
                    expected_trie_data,
                ))
            }
        );

        // Test reorg notification
        let chain_reorg = NewCanonicalChain::Reorg {
            new: vec![block1a.clone(), block2a.clone()],
            old: vec![block1.clone(), block2.clone()],
        };

        // Build expected trie data for old chain
        let mut old_trie_data = BTreeMap::new();
        old_trie_data.insert(1, LazyTrieData::ready(block1.trie_data()));
        old_trie_data.insert(2, LazyTrieData::ready(block2.trie_data()));

        // Build expected trie data for new chain
        let mut new_trie_data = BTreeMap::new();
        new_trie_data.insert(1, LazyTrieData::ready(block1a.trie_data()));
        new_trie_data.insert(2, LazyTrieData::ready(block2a.trie_data()));

        // Build expected execution outcome for reorg chains (first_block matches first block
        // number)
        let reorg_execution_outcome = ExecutionOutcome {
            receipts: vec![vec![], vec![]],
            requests: vec![Requests::default(), Requests::default()],
            first_block: 1,
            ..Default::default()
        };

        assert_eq!(
            chain_reorg.to_chain_notification(),
            CanonStateNotification::Reorg {
                old: Arc::new(Chain::new(
                    vec![block1.recovered_block().clone(), block2.recovered_block().clone()],
                    reorg_execution_outcome.clone(),
                    old_trie_data,
                )),
                new: Arc::new(Chain::new(
                    vec![block1a.recovered_block().clone(), block2a.recovered_block().clone()],
                    reorg_execution_outcome,
                    new_trie_data,
                ))
            }
        );
    }

    #[test]
    fn test_to_chain_notification_carries_prepared_bal() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 = test_block_builder
            .get_executed_block_with_number(1, block0.recovered_block.hash())
            .with_bal(Some(Arc::new(DecodedRevmBal::new(
                Arc::new(revm::state::bal::Bal::default()),
                Bytes::from_static(&[0xc0]),
            ))));

        let chain = NewCanonicalChain::Commit { new: vec![block0, block1.clone()] };
        let CanonStateNotification::Commit { new } = chain.to_chain_notification() else {
            panic!("expected a commit notification")
        };

        // Only the block whose payload carried a BAL contributes one.
        assert_eq!(new.bals().len(), 1);
        assert_eq!(new.bal_at(1), block1.bal());

        let mut blocks_and_bals = new.blocks_and_bals();
        let (block, bal) = blocks_and_bals.next().expect("block with BAL");
        assert_eq!(block.hash(), block1.recovered_block.hash());
        assert_eq!(Some(bal), block1.bal());
        assert!(blocks_and_bals.next().is_none());
    }
}
