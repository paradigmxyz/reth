use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::BlockNumber;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::NodePrimitives;

/// Blocks and persistence frontiers for one forward-persistence operation.
///
/// `blocks` contains every block in `(prev_partial_state_trie, new_db_tip]`, ordered from oldest
/// to newest. This allows the writer to independently persist:
///
/// - non-state/trie data for `(prev_db_tip, new_db_tip]`;
/// - state/trie data for `(prev_partial_state_trie, new_partial_state_trie]`; and
/// - use `(new_partial_state_trie, new_db_tip]` to mask the state/trie writes.
///
/// State/trie refers to canonical hashed state and trie nodes. Append-only bytecodes are part of
/// the non-state/trie range.
#[derive(Debug, Clone)]
pub struct SaveBlocksInput<N: NodePrimitives = EthPrimitives> {
    blocks: Vec<ExecutedBlock<N>>,
    prev_db_tip: BlockNumber,
    prev_partial_state_trie: BlockNumber,
    new_db_tip: BlockNumber,
    new_partial_state_trie: BlockNumber,
}

impl<N: NodePrimitives> SaveBlocksInput<N> {
    /// Creates a new forward-persistence input.
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

    /// Returns the previous partial state/trie tip.
    pub const fn prev_partial_state_trie(&self) -> BlockNumber {
        self.prev_partial_state_trie
    }

    /// Returns the new database tip.
    pub const fn new_db_tip(&self) -> BlockNumber {
        self.new_db_tip
    }

    /// Returns the new partial state/trie tip.
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

    /// Returns `true` if state/trie persistence differs from full block persistence.
    pub const fn is_partial(&self) -> bool {
        self.prev_partial_state_trie != self.prev_db_tip ||
            self.new_partial_state_trie != self.new_db_tip
    }

    /// Returns the blocks whose non-state/trie data should be persisted.
    pub fn blocks_to_persist(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[(self.prev_db_tip - self.prev_partial_state_trie) as usize..]
    }

    /// Returns the blocks whose state/trie data should be persisted.
    pub fn state_trie_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[..(self.new_partial_state_trie - self.prev_partial_state_trie) as usize]
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
                .blocks_to_persist()
                .iter()
                .map(|b| b.recovered_block().number())
                .collect::<Vec<_>>(),
            (12..=22).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .state_trie_blocks()
                .iter()
                .map(|b| b.recovered_block().number())
                .collect::<Vec<_>>(),
            (6..=16).collect::<Vec<_>>()
        );
        assert_eq!(
            input
                .state_trie_masking_blocks()
                .iter()
                .map(|b| b.recovered_block().number())
                .collect::<Vec<_>>(),
            (17..=22).collect::<Vec<_>>()
        );
    }
}
