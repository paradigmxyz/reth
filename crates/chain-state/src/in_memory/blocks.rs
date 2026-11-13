use super::{canonical::InMemoryStateStats, BlockState, ExecutedBlock};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Map, BlockNumber, B256};
use reth_primitives_traits::NodePrimitives;
use std::{collections::BTreeMap, sync::Arc};

/// The canonical blocks that are not on disk yet and the pending block.
///
/// This is everything the lock of the in-memory state guards, so that one update changes the
/// blocks, their numbers and the pending block together.
#[derive(Debug, Default)]
pub(super) struct InMemoryBlocks<N: NodePrimitives> {
    /// The canonical blocks that are not on disk yet.
    canonical: InMemoryCanonicalBlocks<N>,
    /// The pending block that has not yet been made canonical.
    pending: Option<BlockState<N>>,
}

impl<N: NodePrimitives> InMemoryBlocks<N> {
    /// Creates the blocks from the given canonical blocks, their numbers and the pending block.
    pub(super) const fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<BlockNumber, B256>,
        pending: Option<BlockState<N>>,
    ) -> Self {
        Self { canonical: InMemoryCanonicalBlocks { blocks, numbers }, pending }
    }

    /// Returns the canonical blocks.
    pub(super) const fn canonical(&self) -> &InMemoryCanonicalBlocks<N> {
        &self.canonical
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(super) const fn pending_state(&self) -> Option<&BlockState<N>> {
        self.pending.as_ref()
    }

    /// Returns the numbers the metrics of the in-memory state are recorded from.
    pub(super) fn stats(&self) -> InMemoryStateStats {
        let numbers = &self.canonical.numbers;
        InMemoryStateStats {
            earliest: numbers.first_key_value().map(|(number, _)| *number),
            latest: numbers.last_key_value().map(|(number, _)| *number),
            count: numbers.len(),
        }
    }

    /// Updates the pending block with the given block.
    pub(super) fn set_pending_block(&mut self, pending: ExecutedBlock<N>) {
        // fetch the state of the pending block's parent block
        let parent = self.canonical.blocks.get(&pending.recovered_block().parent_hash()).cloned();
        self.pending = Some(BlockState::with_parent(pending, parent));
    }

    /// Removes all reorged blocks, appends the new blocks and connects them to their parent
    /// blocks, and removes the pending block.
    pub(super) fn update_blocks<I, R>(&mut self, new_blocks: I, reorged: R)
    where
        I: IntoIterator<Item = ExecutedBlock<N>>,
        R: IntoIterator<Item = ExecutedBlock<N>>,
    {
        let InMemoryCanonicalBlocks { blocks, numbers } = &mut self.canonical;

        // we first remove the blocks from the reorged chain
        for block in reorged {
            let hash = block.recovered_block().hash();
            let number = block.recovered_block().number();
            blocks.remove(&hash);
            numbers.remove(&number);
        }

        // insert the new blocks
        for block in new_blocks {
            let parent = blocks.get(&block.recovered_block().parent_hash()).cloned();
            let block_state = BlockState::with_parent(block, parent);
            let hash = block_state.hash();
            let number = block_state.number();

            // append new blocks
            blocks.insert(hash, Arc::new(block_state));
            numbers.insert(number, hash);
        }

        // remove the pending state
        self.pending = None;
    }

    /// Removes the blocks through `remove_until`, which is at most the number of the persisted
    /// block, and updates the links between the blocks that stay and of the pending block.
    ///
    /// Does nothing if the persisted block is not one of the canonical blocks.
    pub(super) fn remove_persisted_blocks_until(
        &mut self,
        persisted_num_hash: BlockNumHash,
        remove_until: BlockNumber,
    ) {
        let InMemoryCanonicalBlocks { blocks, numbers } = &mut self.canonical;

        // if the persisted hash is not in the canonical in memory state, do nothing, because it
        // means canonical blocks were not actually persisted.
        //
        // This can happen if the persistence task takes a long time, while a reorg is happening.
        if blocks.get(&persisted_num_hash.hash).is_none() {
            // do nothing
            return
        }

        let remove_until = remove_until.min(persisted_num_hash.number);

        // clear all numbers
        numbers.clear();

        // Drain all blocks and keep only the suffix that still has to stay in memory.
        let mut old_blocks = blocks
            .drain()
            .filter(|(_, b)| b.block_ref().recovered_block().number() > remove_until)
            .map(|(_, b)| b.block.clone())
            .collect::<Vec<_>>();

        // sort the blocks by number so we can insert them back in natural order (low -> high)
        old_blocks.sort_unstable_by_key(|block| block.recovered_block().number());

        // re-insert the blocks in natural order and connect them to their parent blocks
        for block in old_blocks {
            let parent = blocks.get(&block.recovered_block().parent_hash()).cloned();
            let block_state = BlockState::with_parent(block, parent);
            let hash = block_state.hash();
            let number = block_state.number();

            // append new blocks
            blocks.insert(hash, Arc::new(block_state));
            numbers.insert(number, hash);
        }

        // also shift the pending state if it exists
        if let Some(p) = self.pending.as_mut() {
            p.parent = blocks.get(&p.block_ref().recovered_block().parent_hash()).cloned();
        }
    }

    /// Removes all canonical blocks and the pending block.
    pub(super) fn clear(&mut self) {
        self.canonical.numbers.clear();
        self.canonical.blocks.clear();
        self.pending = None;
    }
}

/// The canonical blocks that are not on disk yet.
#[derive(Debug, Default)]
pub(super) struct InMemoryCanonicalBlocks<N: NodePrimitives> {
    /// All canonical blocks that are not on disk yet.
    blocks: B256Map<Arc<BlockState<N>>>,
    /// Mapping of block numbers to block hashes.
    numbers: BTreeMap<BlockNumber, B256>,
}

impl<N: NodePrimitives> InMemoryCanonicalBlocks<N> {
    /// Returns the state for a given block hash.
    pub(super) fn get(&self, hash: &B256) -> Option<&Arc<BlockState<N>>> {
        self.blocks.get(hash)
    }

    /// Returns the state for a given block number.
    pub(super) fn by_number(&self, number: BlockNumber) -> Option<&Arc<BlockState<N>>> {
        self.numbers.get(&number).and_then(|hash| self.blocks.get(hash))
    }

    /// Returns the hash for a specific block number
    pub(super) fn hash_by_number(&self, number: BlockNumber) -> Option<B256> {
        self.numbers.get(&number).copied()
    }

    /// Returns the current chain head state.
    pub(super) fn head(&self) -> Option<&Arc<BlockState<N>>> {
        self.numbers.last_key_value().and_then(|(_, hash)| self.blocks.get(hash))
    }

    /// Returns the number of canonical blocks.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.blocks.len()
    }
}
