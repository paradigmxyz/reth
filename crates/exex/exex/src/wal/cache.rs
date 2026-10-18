use std::{
    cmp::Reverse,
    collections::{binary_heap::PeekMut, BinaryHeap, HashSet},
};

use super::WalResult;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::FbHashMap, BlockNumber, B256};
use reth_exex_types::ExExNotification;
use reth_node_api::NodePrimitives;

#[cfg(test)]
use itertools::Itertools;

/// The block cache of the WAL.
///
/// This cache is needed to avoid walking the WAL directory every time we want to find a
/// notification corresponding to a block or a block corresponding to a hash.
#[derive(Debug, Default)]
pub struct BlockCache {
    /// A min heap of `(Block Number, File ID, Size in Bytes)` tuples.
    ///
    /// Contains one highest block in notification. In a notification with both committed and
    /// reverted chain, the highest block is chosen between both chains. The size recorded at write
    /// or reopen lets finalization account for files that are already missing.
    pub(super) notification_max_blocks: BinaryHeap<Reverse<(BlockNumber, u32, u64)>>,
    /// A mapping of committed blocks `Block Hash -> Block`.
    ///
    /// For each [`ExExNotification::ChainCommitted`] notification, there will be an entry per
    /// block.
    pub(super) committed_blocks: FbHashMap<32, (u32, CachedBlock)>,
    /// Block height of the lowest committed block currently in the cache.
    pub(super) lowest_committed_block_height: Option<BlockNumber>,
    /// Block height of the highest committed block currently in the cache.
    pub(super) highest_committed_block_height: Option<BlockNumber>,
}

impl BlockCache {
    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.notification_max_blocks.is_empty()
    }

    /// Returns the number of blocks in the cache.
    pub(super) fn num_blocks(&self) -> usize {
        self.committed_blocks.len()
    }

    /// Removes notifications with a tip at or below the given block number.
    ///
    /// Calls `remove` before evicting each notification. On failure, reconciles previously removed
    /// notifications and leaves the failed notification and all later ones available for retry.
    pub(super) fn remove_through(
        &mut self,
        block_number: BlockNumber,
        mut remove: impl FnMut(u32, u64) -> WalResult<()>,
    ) -> WalResult<()> {
        let mut file_ids = HashSet::new();
        let mut result = Ok(());

        while let Some(entry) = self.notification_max_blocks.peek_mut() {
            let Reverse((max_block, file_id, size)) = *entry;
            if max_block > block_number {
                break;
            }
            if let Err(error) = remove(file_id, size) {
                result = Err(error);
                break;
            }
            let Reverse((_, file_id, _)) = PeekMut::pop(entry);
            file_ids.insert(file_id);
        }

        if file_ids.is_empty() {
            return result;
        }

        let (mut lowest_committed_block_height, mut highest_committed_block_height) = (None, None);
        self.committed_blocks.retain(|_, (file_id, block)| {
            let retain = !file_ids.contains(file_id);

            if retain {
                lowest_committed_block_height = Some(
                    lowest_committed_block_height
                        .map_or(block.block.number, |lowest| block.block.number.min(lowest)),
                );
                highest_committed_block_height = Some(
                    highest_committed_block_height
                        .map_or(block.block.number, |highest| block.block.number.max(highest)),
                );
            }

            retain
        });
        self.lowest_committed_block_height = lowest_committed_block_height;
        self.highest_committed_block_height = highest_committed_block_height;

        result
    }

    /// Returns the file ID for the notification containing the given committed block hash, if it
    /// exists.
    pub(super) fn get_file_id_by_committed_block_hash(&self, block_hash: &B256) -> Option<u32> {
        self.committed_blocks.get(block_hash).map(|entry| entry.0)
    }

    /// Inserts the blocks from the notification into the cache with the given file ID.
    pub(super) fn insert_notification_blocks_with_file_id<N: NodePrimitives>(
        &mut self,
        file_id: u32,
        size: u64,
        notification: &ExExNotification<N>,
    ) {
        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        let max_block =
            reverted_chain.iter().chain(&committed_chain).map(|chain| chain.tip().number()).max();
        if let Some(max_block) = max_block {
            self.notification_max_blocks.push(Reverse((max_block, file_id, size)));
        }

        if let Some(committed_chain) = &committed_chain {
            for block in committed_chain.blocks().values() {
                let cached_block = CachedBlock {
                    block: (block.number(), block.hash()).into(),
                    parent_hash: block.parent_hash(),
                };
                self.committed_blocks.insert(block.hash(), (file_id, cached_block));
            }

            let first_block_number = committed_chain.first().number();
            self.lowest_committed_block_height = Some(
                self.lowest_committed_block_height
                    .map_or(first_block_number, |lowest| lowest.min(first_block_number)),
            );
            self.highest_committed_block_height = Some(committed_chain.tip().number());
        }
    }

    #[cfg(test)]
    pub(super) fn blocks_sorted(&self) -> Vec<(BlockNumber, u32)> {
        self.notification_max_blocks
            .clone()
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse((block, file_id, _))| (block, file_id))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn committed_blocks_sorted(&self) -> Vec<(B256, u32, CachedBlock)> {
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
