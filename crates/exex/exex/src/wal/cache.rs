use std::collections::VecDeque;

use reth_exex_types::ExExNotification;
use reth_primitives::BlockNumHash;

#[derive(Debug)]
pub(super) struct BlockCache {
    deque: VecDeque<CachedBlock>,
    max_capacity: usize,
}

impl BlockCache {
    /// Creates a new instance of [`BlockCache`] with the given maximum capacity.
    pub(super) fn new(max_capacity: usize) -> Self {
        Self { deque: VecDeque::with_capacity(max_capacity), max_capacity }
    }

    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.deque.is_empty()
    }

    /// Returns a front-to-back iterator.
    pub(super) fn iter(&self) -> std::collections::vec_deque::Iter<'_, CachedBlock> {
        self.deque.iter()
    }

    /// Provides a reference to the first block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn front(&self) -> Option<&CachedBlock> {
        self.deque.front()
    }

    /// Provides a reference to the last block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn back(&self) -> Option<&CachedBlock> {
        self.deque.back()
    }

    /// Removes the first block from the cache and returns it, or `None` if
    /// the cache is empty.
    pub(super) fn pop_front(&mut self) -> Option<CachedBlock> {
        self.deque.pop_front()
    }

    /// Removes the last block from the cache and returns it, or `None` if
    /// the cache is empty.
    pub(super) fn pop_back(&mut self) -> Option<CachedBlock> {
        self.deque.pop_back()
    }

    /// Appends a block to the back of the cache.
    ///
    /// If the cache is full, the oldest block is removed from the front.
    pub(super) fn push_back(&mut self, block: CachedBlock) {
        self.deque.push_back(block);
        if self.deque.len() > self.max_capacity {
            self.deque.pop_front();
        }
    }

    /// Clears the cache, removing all blocks
    pub(super) fn clear(&mut self) {
        self.deque.clear();
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

#[cfg(test)]
mod tests {
    use crate::wal::cache::{BlockCache, CachedBlock, CachedBlockAction};

    #[test]
    fn test_eviction() {
        let mut cache = BlockCache::new(1);

        let cached_block_1 = CachedBlock {
            file_offset: 0,
            action: CachedBlockAction::Commit,
            block: Default::default(),
        };
        cache.push_back(cached_block_1);

        let cached_block_2 = CachedBlock {
            file_offset: 1,
            action: CachedBlockAction::Revert,
            block: Default::default(),
        };
        cache.push_back(cached_block_2);

        assert_eq!(cache.iter().collect::<Vec<_>>(), vec![&cached_block_2]);
    }
}
