use std::collections::{BTreeMap, VecDeque};

use dashmap::DashMap;
use reth_exex_types::ExExNotification;
use reth_primitives::{BlockNumHash, B256};

/// The block cache of the WAL.
///
/// This cache is needed to avoid walking the WAL directory every time we want to find a
/// notification corresponding to a block or a block corresponding to a hash.
#[derive(Debug)]
pub struct BlockCache {
    /// A mapping of `File ID -> List of Blocks`.
    ///
    /// For each notification written to the WAL, there will be an entry per block written to
    /// the cache with the same file ID. I.e. for each notification, there may be multiple blocks
    /// in the cache.
    files: BTreeMap<u64, VecDeque<CachedBlock>>,
    /// A mapping of `Block Hash -> Block`.
    ///
    /// For each [`ExExNotification::ChainCommitted`] notification, there will be an entry per
    /// block.
    blocks: DashMap<B256, CachedBlock>,
}

impl BlockCache {
    /// Creates a new instance of [`BlockCache`].
    pub(super) fn new() -> Self {
        Self { files: BTreeMap::new(), blocks: DashMap::new() }
    }

    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns a front-to-back iterator.
    pub(super) fn iter(&self) -> impl Iterator<Item = (u64, CachedBlock)> + '_ {
        self.files.iter().flat_map(|(k, v)| v.iter().map(move |b| (*k, *b)))
    }

    /// Provides a reference to the first block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn front(&self) -> Option<(u64, CachedBlock)> {
        self.files.first_key_value().and_then(|(k, v)| v.front().map(|b| (*k, *b)))
    }

    /// Provides a reference to the last block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn back(&self) -> Option<(u64, CachedBlock)> {
        self.files.last_key_value().and_then(|(k, v)| v.back().map(|b| (*k, *b)))
    }

    /// Removes the notification with the given file ID.
    pub(super) fn remove_notification(&mut self, key: u64) -> Option<VecDeque<CachedBlock>> {
        self.files.remove(&key)
    }

    /// Pops the first block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_front(&mut self) -> Option<(u64, CachedBlock)> {
        let first_entry = self.files.first_entry()?;
        let key = *first_entry.key();
        let blocks = first_entry.into_mut();
        let first_block = blocks.pop_front().unwrap();
        if blocks.is_empty() {
            self.files.remove(&key);
        }

        Some((key, first_block))
    }

    /// Pops the last block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_back(&mut self) -> Option<(u64, CachedBlock)> {
        let last_entry = self.files.last_entry()?;
        let key = *last_entry.key();
        let blocks = last_entry.into_mut();
        let last_block = blocks.pop_back().unwrap();
        if blocks.is_empty() {
            self.files.remove(&key);
        }

        Some((key, last_block))
    }

    /// Appends a block to the back of the specified file entry.
    pub(super) fn insert(&mut self, file_id: u64, block: CachedBlock) {
        self.files.entry(file_id).or_default().push_back(block);
    }

    /// Inserts the blocks from the notification into the cache with the given file ID.
    ///
    /// First, inserts the reverted blocks (if any), then the committed blocks (if any).
    pub(super) fn insert_notification_blocks_with_file_id(
        &mut self,
        file_id: u64,
        notification: &ExExNotification,
    ) {
        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        if let Some(reverted_chain) = reverted_chain {
            for block in reverted_chain.blocks().values() {
                self.insert(
                    file_id,
                    CachedBlock {
                        action: CachedBlockAction::Revert,
                        block: (block.number, block.hash()).into(),
                        parent_hash: block.parent_hash,
                    },
                );
            }
        }

        if let Some(committed_chain) = committed_chain {
            for block in committed_chain.blocks().values() {
                let cached_block = CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (block.number, block.hash()).into(),
                    parent_hash: block.parent_hash,
                };
                self.insert(file_id, cached_block);
                self.blocks.insert(block.hash(), cached_block);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedBlock {
    pub(super) action: CachedBlockAction,
    /// The block number and hash of the block.
    pub(super) block: BlockNumHash,
    /// The hash of the parent block.
    pub(super) parent_hash: B256,
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
