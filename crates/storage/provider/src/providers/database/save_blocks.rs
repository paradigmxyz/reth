use alloy_eips::BlockNumHash;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::NodePrimitives;
use std::ops::Range;

/// A single persistence step over a contiguous region of [`SaveBlocksPlan::blocks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveBlocksPlanStep {
    /// Range of [`SaveBlocksPlan::blocks`] covered by this step.
    pub block_range: Range<usize>,
    /// Optional range of blocks whose state/trie updates should be used to mask this step's
    /// durable state/trie writes.
    ///
    /// `Some(empty_range)` means persist state/trie without any masking. `None` means skip
    /// durable state/trie persistence for this step.
    pub state_trie_masking_range: Option<Range<usize>>,
    /// Whether to persist non-state/trie data for this step.
    pub persist_rest: bool,
}

impl SaveBlocksPlanStep {
    /// Creates a new persistence step.
    pub const fn new(
        block_range: Range<usize>,
        state_trie_masking_range: Option<Range<usize>>,
        persist_rest: bool,
    ) -> Self {
        Self { block_range, state_trie_masking_range, persist_rest }
    }

    /// Returns `true` if this step persists state/trie data.
    pub const fn persists_state_trie(&self) -> bool {
        self.state_trie_masking_range.is_some()
    }
}

/// Plan for a single `save_blocks` persistence cycle.
#[derive(Debug, Clone)]
pub struct SaveBlocksPlan<N: NodePrimitives = EthPrimitives> {
    /// Canonical blocks covered by this plan.
    pub blocks: Vec<ExecutedBlock<N>>,
    /// Ordered persistence steps over [`Self::blocks`].
    pub steps: Vec<SaveBlocksPlanStep>,
}

impl<N: NodePrimitives> SaveBlocksPlan<N> {
    /// Creates a new save plan.
    pub const fn new(blocks: Vec<ExecutedBlock<N>>, steps: Vec<SaveBlocksPlanStep>) -> Self {
        Self { blocks, steps }
    }

    /// Returns `true` if the plan contains no blocks to persist.
    pub fn is_empty(&self) -> bool {
        self.last_block().is_none()
    }

    /// Returns the highest block covered by this plan.
    pub fn last_block(&self) -> Option<BlockNumHash> {
        let last_index =
            self.steps.iter().rev().find_map(|step| step.block_range.end.checked_sub(1))?;
        self.blocks.get(last_index).map(|block| block.recovered_block().num_hash())
    }

    /// Returns the highest block whose state/trie data is durably persisted by this plan.
    pub fn last_state_trie_block(&self) -> Option<BlockNumHash> {
        let last_index = self
            .steps
            .iter()
            .rev()
            .find(|step| step.persists_state_trie())?
            .block_range
            .end
            .checked_sub(1)?;
        self.blocks.get(last_index).map(|block| block.recovered_block().num_hash())
    }

    /// Returns the contiguous range of blocks whose non-state/trie outputs are persisted.
    pub fn persist_rest_range(&self) -> Option<Range<usize>> {
        let mut ranges =
            self.steps.iter().filter(|step| step.persist_rest).map(|step| &step.block_range);
        let first = ranges.next()?.clone();
        let merged = ranges.fold(first, |mut merged, range| {
            debug_assert_eq!(merged.end, range.start, "persist_rest steps must be contiguous");
            merged.end = range.end;
            merged
        });
        Some(merged)
    }

    /// Returns adjacent state/trie persistence ranges coalesced when they use the same masking
    /// range.
    pub fn coalesced_state_trie_ranges(&self) -> Vec<(Range<usize>, Range<usize>)> {
        let mut ranges: Vec<(Range<usize>, Range<usize>)> = Vec::new();

        for step in self.steps.iter().filter(|step| step.persists_state_trie()) {
            let masking_range =
                step.state_trie_masking_range.clone().expect("checked state/trie persistence step");

            if let Some((block_range, existing_masking_range)) = ranges.last_mut() &&
                block_range.end == step.block_range.start &&
                existing_masking_range.start == masking_range.start &&
                existing_masking_range.end == masking_range.end
            {
                block_range.end = step.block_range.end;
            } else {
                ranges.push((step.block_range.clone(), masking_range));
            }
        }

        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::{SaveBlocksPlan, SaveBlocksPlanStep};
    use reth_ethereum_primitives::EthPrimitives;

    #[test]
    fn coalesces_adjacent_state_trie_ranges_with_same_mask() {
        let plan = SaveBlocksPlan::<EthPrimitives>::new(
            Vec::new(),
            vec![
                SaveBlocksPlanStep::new(0..10, Some(11..21), false),
                SaveBlocksPlanStep::new(10..11, Some(11..21), true),
                SaveBlocksPlanStep::new(11..21, None, true),
            ],
        );

        assert_eq!(plan.coalesced_state_trie_ranges(), vec![(0..11, 11..21)]);
    }

    #[test]
    fn keeps_state_trie_ranges_with_different_masks_separate() {
        let plan = SaveBlocksPlan::<EthPrimitives>::new(
            Vec::new(),
            vec![
                SaveBlocksPlanStep::new(0..2, Some(3..5), false),
                SaveBlocksPlanStep::new(2..3, Some(4..5), true),
            ],
        );

        assert_eq!(plan.coalesced_state_trie_ranges(), vec![(0..2, 3..5), (2..3, 4..5)]);
    }
}
