use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlock};
use std::collections::{btree_map::Entry, BTreeMap, HashMap, HashSet};

/// Contains the Tree of pending blocks that are not executed but buffered
/// While pipeline is is
#[derive(Debug, Default)]
pub struct BlockBuffer {
    /// Blocks ordered by block number inside the BTreeMap.
    ///
    /// Note: BTreeMap is used so that we can remove the finalized old blocks
    /// from the buffer
    blocks: BTreeMap<BlockNumber, HashMap<BlockHash, SealedBlock>>,
    /// Needed for removal of the blocks. and to connect the potential unconnected block
    /// to the connected one.
    parent_to_child: HashMap<BlockHash, HashSet<BlockNumHash>>,
    /*
    // TODO ignore for now, can be added later.
    num_of_blocks: usize,
    /// Max number of blocks.
    max_blocks: usize,
    /// Max number of sibling blocks.
    max_siblings: usize,
     */
}

impl BlockBuffer {
    /// Insert block inside the buffer.
    pub fn insert_block(&mut self, block: SealedBlock) {
        self.parent_to_child.entry(block.parent_hash).or_default().insert(block.num_hash());
        self.blocks.entry(block.number).or_default().insert(block.hash, block);
    }

    /// Clean up the old blocks from the buffer as blocks before finalization are not needed
    /// anymore. We can discard them from the buffer.
    pub fn clean_old_blocks(&mut self, finalized_number: BlockNumber) {
        let mut remove_parent_children = Vec::new();

        // discard all blocks that are before the finalized number.
        while let Some(entry) = self.blocks.first_entry() {
            if *entry.key() > finalized_number {
                break
            }
            let blocks = entry.remove();
            remove_parent_children.extend(
                blocks.into_iter().map(|(hash, block)| BlockNumHash::new(block.number, hash)),
            );
        }

        self.remove_childrens(remove_parent_children);
    }

    /// Remove block from `self.blocks`
    fn remove_from_block(&mut self, block: &BlockNumHash) -> Option<SealedBlock> {
        if let Entry::Occupied(mut entry) = self.blocks.entry(block.number) {
            let ret = entry.get_mut().remove(&block.hash);
            // if set is empty remove block entry.
            if entry.get().is_empty() {
                entry.remove();
            }
            return ret
        };
        None
    }

    /// Return all childs and childs childrens from given blocks. Remove them from buffer.
    fn remove_childrens(
        &mut self,
        mut remove_parent_children: Vec<BlockNumHash>,
    ) -> Vec<SealedBlock> {
        // remove all parent child connection and all the child children blocks that are connected
        // to the discarded parent blocks.
        let mut removed_blocks = Vec::new();
        while let Some(parent_num_hash) = remove_parent_children.pop() {
            // get this child blocks children and add them to the remove list.
            if let Some(parent_childrens) = self.parent_to_child.remove(&parent_num_hash.hash) {
                // remove child from buffer
                for child in parent_childrens.iter() {
                    if let Some(block) = self.remove_from_block(child) {
                        removed_blocks.push(block);
                    }
                }
                remove_parent_children.extend(parent_childrens.into_iter());
            }
        }
        removed_blocks
    }

    /// Get all the children of the block and its child children.
    /// This is used to get all the blocks that are dependent on the block that is included.
    ///
    /// Note: that order of returned blocks is important and the blocks with lower block number
    /// in the chain will come first so that they can be executed in the correct order.
    pub fn take_all_childrens(&mut self, parent: BlockNumHash) -> Vec<SealedBlock> {
        // remove parent block if present
        let mut taken = Vec::new();
        if let Some(block) = self.remove_from_block(&parent) {
            taken.push(block);
        }

        taken.extend(self.remove_childrens(vec![parent]).into_iter());
        taken
    }
}
