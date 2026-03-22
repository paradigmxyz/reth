use crate::tree::metrics::BlockBufferMetrics;
use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockHash, BlockNumber};
use reth_primitives_traits::{Block, SealedBlock};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Contains the tree of pending blocks that cannot be executed due to missing parent.
/// It allows to store unconnected blocks for potential future inclusion.
///
/// The buffer has three main functionalities:
/// * [`BlockBuffer::insert_block`] for inserting blocks inside the buffer.
/// * [`BlockBuffer::remove_block_with_children`] for connecting blocks if the parent gets received
///   and inserted.
/// * [`BlockBuffer::remove_old_blocks`] to remove old blocks that precede the finalized number.
///
/// Note: Buffer is limited by number of blocks that it can contain and eviction of the block
/// is done in FIFO order (oldest inserted block is evicted first).
#[derive(Debug)]
pub struct BlockBuffer<B: Block> {
    /// All blocks in the buffer stored by their block hash.
    pub(crate) blocks: HashMap<BlockHash, SealedBlock<B>>,
    /// Map of any parent block hash (even the ones not currently in the buffer)
    /// to the buffered children.
    /// Allows connecting buffered blocks by parent.
    pub(crate) parent_to_child: HashMap<BlockHash, HashSet<BlockHash>>,
    /// `BTreeMap` tracking the earliest blocks by block number.
    /// Used for removal of old blocks that precede finalization.
    pub(crate) earliest_blocks: BTreeMap<BlockNumber, HashSet<BlockHash>>,
    /// FIFO queue tracking block insertion order for eviction.
    /// When the buffer reaches its capacity limit, the oldest block is evicted first.
    pub(crate) block_queue: VecDeque<BlockHash>,
    /// Maximum number of blocks that can be stored in the buffer
    pub(crate) max_blocks: usize,
    /// Various metrics for the block buffer.
    pub(crate) metrics: BlockBufferMetrics,
}

impl<B: Block> BlockBuffer<B> {
    /// Create new buffer with max limit of blocks
    pub fn new(limit: u32) -> Self {
        Self {
            blocks: Default::default(),
            parent_to_child: Default::default(),
            earliest_blocks: Default::default(),
            block_queue: VecDeque::default(),
            max_blocks: limit as usize,
            metrics: Default::default(),
        }
    }

    /// Return reference to the requested block.
    pub fn block(&self, hash: &BlockHash) -> Option<&SealedBlock<B>> {
        self.blocks.get(hash)
    }

    /// Return a reference to the lowest ancestor of the given block in the buffer.
    pub fn lowest_ancestor(&self, hash: &BlockHash) -> Option<&SealedBlock<B>> {
        let mut current_block = self.blocks.get(hash)?;
        while let Some(parent) = self.blocks.get(&current_block.parent_hash()) {
            current_block = parent;
        }
        Some(current_block)
    }

    /// Insert a correct block inside the buffer.
    pub fn insert_block(&mut self, block: SealedBlock<B>) {
        let hash = block.hash();

        match self.blocks.entry(hash) {
            std::collections::hash_map::Entry::Occupied(_) => return,
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.parent_to_child.entry(block.parent_hash()).or_default().insert(hash);
                self.earliest_blocks.entry(block.number()).or_default().insert(hash);
                entry.insert(block);
            }
        };

        // Add block to FIFO queue and handle eviction if needed
        if self.block_queue.len() >= self.max_blocks {
            // Evict oldest block if limit is hit
            if let Some(evicted_hash) = self.block_queue.pop_front() {
                self.remove_block(&evicted_hash);
            }
        }
        self.block_queue.push_back(hash);
        self.metrics.blocks.set(self.blocks.len() as f64);
    }

    /// Removes the given block from the buffer and also all the children of the block.
    ///
    /// This is used to get all the blocks that are dependent on the block that is included.
    ///
    /// Note: that order of returned blocks is important and the blocks with lower block number
    /// in the chain will come first so that they can be executed in the correct order.
    pub fn remove_block_with_children(&mut self, parent_hash: &BlockHash) -> Vec<SealedBlock<B>> {
        let removed = self
            .remove_block(parent_hash)
            .into_iter()
            .chain(self.remove_children(vec![*parent_hash]))
            .collect();
        self.metrics.blocks.set(self.blocks.len() as f64);
        removed
    }

    /// Discard all blocks that precede block number from the buffer.
    pub fn remove_old_blocks(&mut self, block_number: BlockNumber) {
        let mut block_hashes_to_remove = Vec::new();

        // discard all blocks that are before the finalized number.
        while let Some(entry) = self.earliest_blocks.first_entry() {
            if *entry.key() > block_number {
                break
            }
            let block_hashes = entry.remove();
            block_hashes_to_remove.extend(block_hashes);
        }

        // remove from other collections.
        for block_hash in &block_hashes_to_remove {
            // It's fine to call
            self.remove_block(block_hash);
        }

        self.remove_children(block_hashes_to_remove);
        self.metrics.blocks.set(self.blocks.len() as f64);
    }

    /// Remove block entry
    fn remove_from_earliest_blocks(&mut self, number: BlockNumber, hash: &BlockHash) {
        if let Some(entry) = self.earliest_blocks.get_mut(&number) {
            entry.remove(hash);
            if entry.is_empty() {
                self.earliest_blocks.remove(&number);
            }
        }
    }

    /// Remove from parent child connection. This method does not remove children.
    fn remove_from_parent(&mut self, parent_hash: BlockHash, hash: &BlockHash) {
        // remove from parent to child connection, but only for this block parent.
        if let Some(entry) = self.parent_to_child.get_mut(&parent_hash) {
            entry.remove(hash);
            // if set is empty remove block entry.
            if entry.is_empty() {
                self.parent_to_child.remove(&parent_hash);
            }
        }
    }

    /// Removes block from inner collections.
    /// This method will only remove the block if it's present inside `self.blocks`.
    /// The block might be missing from other collections, the method will only ensure that it has
    /// been removed.
    fn remove_block(&mut self, hash: &BlockHash) -> Option<SealedBlock<B>> {
        let block = self.blocks.remove(hash)?;
        self.remove_from_earliest_blocks(block.number(), hash);
        self.remove_from_parent(block.parent_hash(), hash);
        self.block_queue.retain(|h| h != hash);
        Some(block)
    }

    /// Remove all children and their descendants for the given blocks and return them.
    fn remove_children(&mut self, parent_hashes: Vec<BlockHash>) -> Vec<SealedBlock<B>> {
        // remove all parent child connection and all the child children blocks that are connected
        // to the discarded parent blocks.
        let mut remove_parent_children = parent_hashes;
        let mut removed_blocks = Vec::new();
        while let Some(parent_hash) = remove_parent_children.pop() {
            // get this child blocks children and add them to the remove list.
            if let Some(parent_children) = self.parent_to_child.remove(&parent_hash) {
                // remove child from buffer
                for child_hash in &parent_children {
                    if let Some(block) = self.remove_block(child_hash) {
                        removed_blocks.push(block);
                    }
                }
                remove_parent_children.extend(parent_children);
            }
        }
        removed_blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::BlockNumHash;
    use alloy_primitives::BlockHash;
    use reth_testing_utils::generators::{self, random_block, BlockParams, Rng};
    use std::collections::HashMap;

    /// Create random block with specified number and parent hash.
    fn create_block<R: Rng>(
        rng: &mut R,
        number: u64,
        parent: BlockHash,
    ) -> SealedBlock<reth_ethereum_primitives::Block> {
        random_block(rng, number, BlockParams { parent: Some(parent), ..Default::default() })
    }

    /// Assert that all buffer collections have the same data length.
    fn assert_buffer_lengths<B: Block>(buffer: &BlockBuffer<B>, expected: usize) {
        assert_eq!(buffer.blocks.len(), expected);
        assert_eq!(buffer.block_queue.len(), expected);
        assert_eq!(
            buffer.parent_to_child.iter().fold(0, |acc, (_, hashes)| acc + hashes.len()),
            expected
        );
        assert_eq!(
            buffer.earliest_blocks.iter().fold(0, |acc, (_, hashes)| acc + hashes.len()),
            expected
        );
    }

    /// Assert that the block was removed from all buffer collections.
    fn assert_block_removal<B: Block>(
        buffer: &BlockBuffer<B>,
        block: &SealedBlock<reth_ethereum_primitives::Block>,
    ) {
        assert!(!buffer.blocks.contains_key(&block.hash()));
        assert!(buffer
            .parent_to_child
            .get(&block.parent_hash)
            .and_then(|p| p.get(&block.hash()))
            .is_none());
        assert!(buffer
            .earliest_blocks
            .get(&block.number)
            .and_then(|hashes| hashes.get(&block.hash()))
            .is_none());
    }

    #[test]
    fn simple_insertion() {
        let mut rng = generators::rng();
        let parent = rng.random();
        let block1 = create_block(&mut rng, 10, parent);
        let mut buffer = BlockBuffer::new(3);

        buffer.insert_block(block1.clone());
        assert_buffer_lengths(&buffer, 1);
        assert_eq!(buffer.block(&block1.hash()), Some(&block1));
    }

    #[test]
    fn take_entire_chain_of_children() {
        let mut rng = generators::rng();

        let main_parent_hash = rng.random();
        let block1 = create_block(&mut rng, 10, main_parent_hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 12, block2.hash());
        let parent4 = rng.random();
        let block4 = create_block(&mut rng, 14, parent4);

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_buffer_lengths(&buffer, 4);
        assert_eq!(buffer.block(&block4.hash()), Some(&block4));
        assert_eq!(buffer.block(&block2.hash()), Some(&block2));
        assert_eq!(buffer.block(&main_parent_hash), None);

        assert_eq!(buffer.lowest_ancestor(&block4.hash()), Some(&block4));
        assert_eq!(buffer.lowest_ancestor(&block3.hash()), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block1.hash()), Some(&block1));
        assert_eq!(
            buffer.remove_block_with_children(&main_parent_hash),
            vec![block1, block2, block3]
        );
        assert_buffer_lengths(&buffer, 1);
    }

    #[test]
    fn take_all_multi_level_children() {
        let mut rng = generators::rng();

        let main_parent_hash = rng.random();
        let block1 = create_block(&mut rng, 10, main_parent_hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 11, block1.hash());
        let block4 = create_block(&mut rng, 12, block2.hash());

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_buffer_lengths(&buffer, 4);
        assert_eq!(
            buffer
                .remove_block_with_children(&main_parent_hash)
                .into_iter()
                .map(|b| (b.hash(), b))
                .collect::<HashMap<_, _>>(),
            HashMap::from([
                (block1.hash(), block1),
                (block2.hash(), block2),
                (block3.hash(), block3),
                (block4.hash(), block4)
            ])
        );
        assert_buffer_lengths(&buffer, 0);
    }

    #[test]
    fn take_block_with_children() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 11, block1.hash());
        let block4 = create_block(&mut rng, 12, block2.hash());

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_buffer_lengths(&buffer, 4);
        assert_eq!(
            buffer
                .remove_block_with_children(&block1.hash())
                .into_iter()
                .map(|b| (b.hash(), b))
                .collect::<HashMap<_, _>>(),
            HashMap::from([
                (block1.hash(), block1),
                (block2.hash(), block2),
                (block3.hash(), block3),
                (block4.hash(), block4)
            ])
        );
        assert_buffer_lengths(&buffer, 0);
    }

    #[test]
    fn remove_chain_of_children() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 12, block2.hash());
        let parent4 = rng.random();
        let block4 = create_block(&mut rng, 14, parent4);

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        assert_buffer_lengths(&buffer, 4);
        buffer.remove_old_blocks(block1.number);
        assert_buffer_lengths(&buffer, 1);
    }

    #[test]
    fn remove_all_multi_level_children() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 11, block1.hash());
        let block4 = create_block(&mut rng, 12, block2.hash());

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        assert_buffer_lengths(&buffer, 4);
        buffer.remove_old_blocks(block1.number);
        assert_buffer_lengths(&buffer, 0);
    }

    #[test]
    fn remove_multi_chains() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block1a = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block2a = create_block(&mut rng, 11, block1.hash());
        let random_parent1 = rng.random();
        let random_block1 = create_block(&mut rng, 10, random_parent1);
        let random_parent2 = rng.random();
        let random_block2 = create_block(&mut rng, 11, random_parent2);
        let random_parent3 = rng.random();
        let random_block3 = create_block(&mut rng, 12, random_parent3);

        let mut buffer = BlockBuffer::new(10);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block1a.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block2a.clone());
        buffer.insert_block(random_block1.clone());
        buffer.insert_block(random_block2.clone());
        buffer.insert_block(random_block3.clone());

        // check that random blocks are their own ancestor, and that chains have proper ancestors
        assert_eq!(buffer.lowest_ancestor(&random_block1.hash()), Some(&random_block1));
        assert_eq!(buffer.lowest_ancestor(&random_block2.hash()), Some(&random_block2));
        assert_eq!(buffer.lowest_ancestor(&random_block3.hash()), Some(&random_block3));

        // descendants have ancestors
        assert_eq!(buffer.lowest_ancestor(&block2a.hash()), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block2.hash()), Some(&block1));

        // roots are themselves
        assert_eq!(buffer.lowest_ancestor(&block1a.hash()), Some(&block1a));
        assert_eq!(buffer.lowest_ancestor(&block1.hash()), Some(&block1));

        assert_buffer_lengths(&buffer, 7);
        buffer.remove_old_blocks(10);
        assert_buffer_lengths(&buffer, 2);
    }

    #[test]
    fn evict_with_gap() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 12, block2.hash());
        let parent4 = rng.random();
        let block4 = create_block(&mut rng, 13, parent4);

        let mut buffer = BlockBuffer::new(3);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());

        // pre-eviction block1 is the root
        assert_eq!(buffer.lowest_ancestor(&block3.hash()), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block2.hash()), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block1.hash()), Some(&block1));

        buffer.insert_block(block4.clone());

        assert_eq!(buffer.lowest_ancestor(&block4.hash()), Some(&block4));

        // block1 gets evicted
        assert_block_removal(&buffer, &block1);

        // check lowest ancestor results post eviction
        assert_eq!(buffer.lowest_ancestor(&block3.hash()), Some(&block2));
        assert_eq!(buffer.lowest_ancestor(&block2.hash()), Some(&block2));
        assert_eq!(buffer.lowest_ancestor(&block1.hash()), None);

        assert_buffer_lengths(&buffer, 3);
    }

    #[test]
    fn simple_eviction() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        let block3 = create_block(&mut rng, 12, block2.hash());
        let parent4 = rng.random();
        let block4 = create_block(&mut rng, 13, parent4);

        let mut buffer = BlockBuffer::new(3);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        // block3 gets evicted
        assert_block_removal(&buffer, &block1);

        assert_buffer_lengths(&buffer, 3);
    }

    #[test]
    fn eviction_parent_child_cleanup() {
        let mut rng = generators::rng();

        let main_parent = BlockNumHash::new(9, rng.random());
        let block1 = create_block(&mut rng, 10, main_parent.hash);
        let block2 = create_block(&mut rng, 11, block1.hash());
        // Unrelated block to trigger eviction
        let unrelated_parent = rng.random();
        let unrelated_block = create_block(&mut rng, 12, unrelated_parent);

        // Capacity 2 so third insert evicts the oldest (block1)
        let mut buffer = BlockBuffer::new(2);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());

        // Pre-eviction: parent_to_child contains main_parent -> {block1}, block1 -> {block2}
        assert!(buffer
            .parent_to_child
            .get(&main_parent.hash)
            .and_then(|s| s.get(&block1.hash()))
            .is_some());
        assert!(buffer
            .parent_to_child
            .get(&block1.hash())
            .and_then(|s| s.get(&block2.hash()))
            .is_some());

        // Insert unrelated block to evict block1
        buffer.insert_block(unrelated_block);

        // Evicted block1 should be fully removed from collections
        assert_block_removal(&buffer, &block1);

        // Cleanup: parent_to_child must no longer have (main_parent -> block1)
        assert!(buffer
            .parent_to_child
            .get(&main_parent.hash)
            .and_then(|s| s.get(&block1.hash()))
            .is_none());

        // But the mapping (block1 -> block2) must remain so descendants can still be tracked
        assert!(buffer
            .parent_to_child
            .get(&block1.hash())
            .and_then(|s| s.get(&block2.hash()))
            .is_some());

        // And lowest ancestor for block2 becomes itself after its parent is evicted
        assert_eq!(buffer.lowest_ancestor(&block2.hash()), Some(&block2));
    }
}
