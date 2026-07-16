use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::BlockNumber;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::NodePrimitives;

/// Input for advancing the engine's two persistence frontiers.
///
/// The Finish checkpoint's block number (`db_tip`) tracks complete block, execution, and history
/// data. `Finish.partial_state_trie` tracks the hashed-state/trie frontier, which may lag behind it
/// while a suffix of blocks is retained in memory as a mask. The hashed-state/trie tables are not
/// a complete snapshot at `db_tip` while this suffix exists; the database and in-memory mask
/// together represent the canonical state.
///
/// `blocks` contains every canonical block in `(prev_partial_state_trie, new_db_tip]`, ordered from
/// oldest to newest. The four frontiers derive the ranges written during this operation:
///
/// - `(prev_db_tip, new_db_tip]`: persist block, execution, and history data;
/// - `(prev_partial_state_trie, new_partial_state_trie]`: consider hashed-state/trie updates for
///   persistence; and
/// - `(new_partial_state_trie, new_db_tip]`: retain as the masking suffix and do not persist its
///   hashed-state/trie updates.
///
/// Updates in the second range that are overwritten by the masking suffix are also omitted. This
/// is the work partial persistence avoids: only the newest value needs to be applied later when
/// the masking block leaves memory. If both new frontiers are equal, the masking suffix is empty
/// and hashed state/trie are fully flushed through `new_db_tip`.
#[derive(Debug, Clone)]
pub struct SaveBlocksInput<N: NodePrimitives = EthPrimitives> {
    blocks: Vec<ExecutedBlock<N>>,
    prev_db_tip: BlockNumber,
    prev_partial_state_trie: BlockNumber,
    new_db_tip: BlockNumber,
    new_partial_state_trie: BlockNumber,
}

impl<N: NodePrimitives> SaveBlocksInput<N> {
    /// Creates an input that advances the existing frontiers to `new_db_tip` and
    /// `new_partial_state_trie`.
    ///
    /// `new_partial_state_trie` may remain behind `prev_db_tip`. This occurs while a masking
    /// window is first growing: new block data advances without forcing all previously masked
    /// hashed-state/trie updates to disk.
    ///
    /// # Panics
    ///
    /// Panics if either frontier moves backwards, the state/trie frontier exceeds the database
    /// frontier, or `blocks` is not the exact contiguous range
    /// `(prev_partial_state_trie, new_db_tip]`.
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

    /// Returns the canonical blocks needed to advance either frontier or construct the mask.
    pub fn blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks
    }

    /// Returns the previous Finish checkpoint block number.
    pub const fn prev_db_tip(&self) -> BlockNumber {
        self.prev_db_tip
    }

    /// Returns the previous `Finish.partial_state_trie` frontier.
    pub const fn prev_partial_state_trie(&self) -> BlockNumber {
        self.prev_partial_state_trie
    }

    /// Returns the new Finish checkpoint block number.
    pub const fn new_db_tip(&self) -> BlockNumber {
        self.new_db_tip
    }

    /// Returns the new `Finish.partial_state_trie` frontier.
    pub const fn new_partial_state_trie(&self) -> BlockNumber {
        self.new_partial_state_trie
    }

    /// Returns the block at the new Finish checkpoint.
    pub fn last_block(&self) -> Option<BlockNumHash> {
        self.blocks.last().map(|block| block.recovered_block().num_hash())
    }

    /// Returns `true` if neither persistence frontier advances.
    pub const fn is_empty(&self) -> bool {
        self.prev_db_tip == self.new_db_tip &&
            self.prev_partial_state_trie == self.new_partial_state_trie
    }

    /// Returns newly persisted blocks whose block, execution, and history data should be written.
    pub fn persist_rest_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[(self.prev_db_tip - self.prev_partial_state_trie) as usize..]
    }

    /// Returns all blocks whose hashed-state/trie updates are candidates for persistence.
    ///
    /// Updates overwritten by [`Self::state_trie_masking_blocks`] are filtered out by the writer.
    pub fn state_trie_blocks(&self) -> &[ExecutedBlock<N>] {
        &self.blocks[..(self.new_partial_state_trie - self.prev_partial_state_trie) as usize]
    }

    /// Returns previously masked blocks that have aged out of the window.
    ///
    /// Their block and execution data is already durable, so only their non-masked
    /// hashed-state/trie updates need to catch up to disk.
    pub fn state_trie_catchup_blocks(&self) -> &[ExecutedBlock<N>] {
        let catchup_tip = self.prev_db_tip.min(self.new_partial_state_trie);
        &self.blocks[..(catchup_tip - self.prev_partial_state_trie) as usize]
    }

    /// Returns newly persisted blocks before the new masking suffix.
    ///
    /// These blocks need both their ordinary data and their non-masked hashed-state/trie updates
    /// written during this operation.
    pub fn new_state_trie_blocks(&self) -> &[ExecutedBlock<N>] {
        let start = self.prev_db_tip.min(self.new_partial_state_trie);
        &self.blocks[(start - self.prev_partial_state_trie) as usize..
            (self.new_partial_state_trie - self.prev_partial_state_trie) as usize]
    }

    /// Returns the fixed suffix whose hashed-state/trie updates remain in memory.
    ///
    /// This suffix suppresses older updates to the same hashed keys and trie nodes. It is also the
    /// overlay required to use the database at the new Finish checkpoint.
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
