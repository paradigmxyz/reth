use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
};

use alloy_eips::BlockNumHash;
use alloy_primitives::{map::FbHashMap, BlockNumber, B256};
use reth_exex_types::ExExNotification;

/// The block cache of the WAL.
///
/// This cache is needed to avoid walking the WAL directory every time we want to find a
/// notification corresponding to a block or a block corresponding to a hash.
#[derive(Debug, Default)]
pub struct BlockCache {
    /// A min heap of `(Block Number, File ID)` tuples.
    pub(super) blocks: BinaryHeap<Reverse<(BlockNumber, u32)>>,
    /// A mapping of committed blocks `Block Hash -> Block`.
    ///
    /// For each [`ExExNotification::ChainCommitted`] notification, there will be an entry per
    /// block.
    pub(super) committed_blocks: FbHashMap<32, (u32, CachedBlock)>,
}

impl BlockCache {
    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Removes all files from the cache that has notifications with a tip block less than or equal
    /// to the given block number.
    ///
    /// # Returns
    ///
    /// A set of file IDs that were removed.
    pub(super) fn remove_before(&mut self, block_number: BlockNumber) -> HashSet<u32> {
        let mut file_ids = HashSet::default();

        while let Some(block @ Reverse((max_block, file_id))) = self.blocks.peek().copied() {
            if max_block <= block_number {
                let popped_block = self.blocks.pop().unwrap();
                debug_assert_eq!(popped_block, block);
                file_ids.insert(file_id);
            } else {
                break
            }
        }

        self.committed_blocks.retain(|_, (file_id, _)| !file_ids.contains(file_id));

        file_ids
    }

    /// Returns the file ID for the notification containing the given committed block hash, if it
    /// exists.
    pub(super) fn get_file_id_by_committed_block_hash(&self, block_hash: &B256) -> Option<u32> {
        self.committed_blocks.get(block_hash).map(|entry| entry.0)
    }

    /// Inserts the blocks from the notification into the cache with the given file ID.
    pub(super) fn insert_notification_blocks_with_file_id(
        &mut self,
        file_id: u32,
        notification: &ExExNotification,
    ) {
        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        let max_block =
            reverted_chain.iter().chain(&committed_chain).map(|chain| chain.tip().number).max();
        if let Some(max_block) = max_block {
            self.blocks.push(Reverse((max_block, file_id)));
        }

        if let Some(committed_chain) = &committed_chain {
            for block in committed_chain.blocks().values() {
                let cached_block = CachedBlock {
                    block: (block.number, block.hash()).into(),
                    parent_hash: block.parent_hash,
                };
                self.committed_blocks.insert(block.hash(), (file_id, cached_block));
            }
        }
    }

    #[cfg(test)]
    pub(super) fn blocks_sorted(&self) -> Vec<(BlockNumber, u32)> {
        self.blocks.clone().into_sorted_vec().into_iter().map(|entry| entry.0).collect()
    }

    #[cfg(test)]
    pub(super) fn committed_blocks_sorted(&self) -> Vec<(B256, u32, CachedBlock)> {
        use itertools::Itertools;

        self.committed_blocks
            .iter()
            .map(|(hash, (file_id, block))| (*hash, *file_id, *block))
            .sorted_by_key(|(_, _, block)| (block.block.number, block.block.hash))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedBlock {
    /// The block number and hash of the block.
    pub(super) block: BlockNumHash,
    /// The hash of the parent block.
    pub(super) parent_hash: B256,
}
