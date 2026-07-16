use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::BlockNumber;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::NodePrimitives;

/// Blocks and persistence frontiers for one partial-persistence operation.
///
/// `blocks` contains every block in `(prev_partial_state_trie, new_db_tip]`, ordered from oldest
/// to newest. The frontiers split that range into the three slices needed by the writer:
///
/// - non-state/trie data for `(prev_db_tip, new_db_tip]`;
/// - state/trie data for `(prev_partial_state_trie, new_partial_state_trie]`; and
/// - masking state/trie data for `(new_partial_state_trie, new_db_tip]`.
#[derive(Debug, Clone)]
pub struct SaveBlocksInput<N: NodePrimitives = EthPrimitives> {
    blocks: Vec<ExecutedBlock<N>>,
    prev_db_tip: BlockNumber,
    prev_partial_state_trie: BlockNumber,
    new_db_tip: BlockNumber,
    new_partial_state_trie: BlockNumber,
}

impl<N: NodePrimitives> SaveBlocksInput<N> {
    /// Creates a new partial-persistence input.
    pub fn new(
        blocks: Vec<ExecutedBlock<N>>,
        prev_db_tip: BlockNumber,
        prev_partial_state_trie: BlockNumber,
        new_db_tip: BlockNumber,
        new_partial_state_trie: BlockNumber,
    ) -> Self {
        assert!(
            prev_partial_state_trie <= prev_db_tip,
            "previous state/trie tip must not exceed previous database tip"
        );
        assert!(prev_db_tip <= new_db_tip, "database tip must not move backwards");
        assert!(
            prev_partial_state_trie <= new_partial_state_trie,
            "state/trie tip must not move backwards"
        );
        assert!(
            new_partial_state_trie <= new_db_tip,
            "new state/trie tip must not exceed new database tip"
        );

        let expected_len = new_db_tip.saturating_sub(prev_partial_state_trie) as usize;
        assert_eq!(
            blocks.len(),
            expected_len,
            "blocks must cover (prev_partial_state_trie, new_db_tip]"
        );
        debug_assert!(blocks.iter().enumerate().all(|(index, block)| {
            block.recovered_block().number() == prev_partial_state_trie + index as u64 + 1
        }));

        Self { blocks, prev_db_tip, prev_partial_state_trie, new_db_tip, new_partial_state_trie }
    }

    /// Returns all blocks carried by this input.
    pub fn blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks
    }

    /// Returns the previous database tip.
    pub const fn prev_db_tip(&self) -> BlockNumber {
        self.prev_db_tip
    }

    /// Returns the previous state/trie tip.
    pub const fn prev_partial_state_trie(&self) -> BlockNumber {
        self.prev_partial_state_trie
    }

    /// Returns the new database tip.
    pub const fn new_db_tip(&self) -> BlockNumber {
        self.new_db_tip
    }

    /// Returns the new state/trie tip.
    pub const fn new_partial_state_trie(&self) -> BlockNumber {
        self.new_partial_state_trie
    }

    /// Returns the new database tip block.
    pub fn last_block(&self) -> Option<BlockNumHash> {
        self.blocks.last().map(|block| block.recovered_block().num_hash())
    }

    /// Returns `true` if neither persistence frontier advances.
    pub const fn is_empty(&self) -> bool {
        self.prev_db_tip == self.new_db_tip &&
            self.prev_partial_state_trie == self.new_partial_state_trie
    }

    /// Returns the blocks whose non-state/trie data should be persisted.
    pub fn persist_rest_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[(self.prev_db_tip - self.prev_partial_state_trie) as usize..]
    }

    /// Returns the blocks whose state/trie data should be persisted.
    pub fn state_trie_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[..(self.new_partial_state_trie - self.prev_partial_state_trie) as usize]
    }

    /// Returns previously masked blocks whose state/trie data should catch up to disk.
    pub fn state_trie_catchup_blocks(&self) -> &[ExecutedBlock<N>] {
        let catchup_tip = self.prev_db_tip.min(self.new_partial_state_trie);
        &self.blocks[..(catchup_tip - self.prev_partial_state_trie) as usize]
    }

    /// Returns newly block-persisted blocks whose state/trie data should also be persisted.
    pub fn new_state_trie_blocks(&self) -> &[ExecutedBlock<N>] {
        let start = self.prev_db_tip.min(self.new_partial_state_trie);
        &self.blocks[(start - self.prev_partial_state_trie) as usize..
            (self.new_partial_state_trie - self.prev_partial_state_trie) as usize]
    }

    /// Returns the blocks that mask state/trie writes.
    pub fn state_trie_masking_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[(self.new_partial_state_trie - self.prev_partial_state_trie) as usize..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chain_state::test_utils::TestBlockBuilder;

    #[test]
    fn splits_frontier_ranges() {
        let blocks = TestBlockBuilder::eth().get_executed_blocks(6..23).collect();
        let input = SaveBlocksInput::new(blocks, 11, 5, 22, 16);

        assert_eq!(
            input
                .persist_rest_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            (12..=22).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .state_trie_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            (6..=16).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .state_trie_catchup_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            (6..=11).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .new_state_trie_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            (12..=16).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .state_trie_masking_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            (17..=22).collect::<Vec<_>>()
        );
    }

    #[test]
    fn grows_masking_window_without_forcing_new_state_writes() {
        let blocks = TestBlockBuilder::eth().get_executed_blocks(13..17).collect();
        let input = SaveBlocksInput::new(blocks, 15, 12, 16, 13);

        assert_eq!(input.persist_rest_blocks()[0].recovered_block().number(), 16);
        assert_eq!(input.state_trie_catchup_blocks()[0].recovered_block().number(), 13);
        assert!(input.new_state_trie_blocks().is_empty());
        assert_eq!(
            input
                .state_trie_masking_blocks()
                .iter()
                .map(|block| block.recovered_block().number())
                .collect::<Vec<_>>(),
            vec![14, 15, 16]
        );
    }
}
