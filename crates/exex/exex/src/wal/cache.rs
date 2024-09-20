use std::collections::VecDeque;

use reth_exex_types::ExExNotification;
use reth_primitives::BlockNumHash;

/// The maximum number of blocks to cache.
///
/// [`CachedBlock`] has a size of `u64 + u64 + B256` which is 384 bits. 384 bits * 1 million = 48
/// megabytes.
const MAX_CACHED_BLOCKS: usize = 1_000_000;

#[derive(Debug, Clone, Default)]
pub(super) struct BlockCache(VecDeque<CachedBlock>);

impl BlockCache {
    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns a front-to-back iterator.
    pub(super) fn iter(&self) -> std::collections::vec_deque::Iter<'_, CachedBlock> {
        self.0.iter()
    }

    /// Provides a reference to the first block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn front(&self) -> Option<&CachedBlock> {
        self.0.front()
    }

    /// Provides a reference to the last block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn back(&self) -> Option<&CachedBlock> {
        self.0.back()
    }

    /// Removes the first block from the cache and returns it, or `None` if
    /// the cache is empty.
    pub(super) fn pop_front(&mut self) -> Option<CachedBlock> {
        self.0.pop_front()
    }

    /// Removes the last block from the cache and returns it, or `None` if
    /// the cache is empty.
    pub(super) fn pop_back(&mut self) -> Option<CachedBlock> {
        self.0.pop_back()
    }

    /// Appends a block to the back of the cache.
    ///
    /// If the cache is full, the oldest block is removed from the front.
    pub(super) fn push_back(&mut self, block: CachedBlock) {
        self.0.push_back(block);
        if self.0.len() > MAX_CACHED_BLOCKS {
            self.0.pop_front();
        }
    }

    /// Clears the cache, removing all blocks
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// Inserts the blocks from the notification into the cache at the given file offset.
    ///
    /// First, inserts the reverted blocks (if any), then the committed blocks (if any).
    pub(super) fn insert_notification_blocks_at_offset(
        &mut self,
        notification: &ExExNotification,
        file_offset: u64,
    ) {
        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        if let Some(reverted_chain) = reverted_chain {
            for block in reverted_chain.blocks().values() {
                self.push_back(CachedBlock {
                    file_offset,
                    action: CachedBlockAction::Revert,
                    block: (block.number, block.hash()).into(),
                });
            }
        }

        if let Some(committed_chain) = committed_chain {
            for block in committed_chain.blocks().values() {
                self.push_back(CachedBlock {
                    file_offset,
                    action: CachedBlockAction::Commit,
                    block: (block.number, block.hash()).into(),
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedBlock {
    /// The file offset where the WAL entry is written.
    pub(super) file_offset: u64,
    pub(super) action: CachedBlockAction,
    /// The block number and hash of the block.
    pub(super) block: BlockNumHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachedBlockAction {
    Commit,
    Revert,
}

impl CachedBlockAction {
    pub(super) const fn is_commit(&self) -> bool {
        matches!(self, Self::Commit)
    }
}
