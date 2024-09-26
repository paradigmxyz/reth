use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use reth_exex_types::ExExNotification;
use reth_primitives::BlockNumHash;

/// The block cache of the WAL. Acts as a mapping of `File ID -> List of Blocks`.
///
/// For each notification written to the WAL, there will be an entry per block written to
/// the cache with the same file ID. I.e. for each notification, there may be multiple blocks in the
/// cache.
///
/// This cache is needed to avoid walking the WAL directory every time we want to find a
/// notification corresponding to a block.
#[derive(Debug, Clone)]
pub struct BlockCache(Arc<RwLock<BTreeMap<u64, VecDeque<CachedBlock>>>>);

impl BlockCache {
    /// Creates a new instance of [`BlockCache`].
    pub(super) fn new() -> Self {
        Self(Arc::new(RwLock::new(BTreeMap::new())))
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<u64, VecDeque<CachedBlock>>> {
        self.0.read().expect("BlockCache is poisoned")
    }

    fn write(&self) -> RwLockWriteGuard<'_, BTreeMap<u64, VecDeque<CachedBlock>>> {
        self.0.write().expect("BlockCache is poisoned")
    }

    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Returns a front-to-back iterator.
    pub(super) fn iter(&self) -> impl Iterator<Item = (u64, CachedBlock)> + '_ {
        self.read()
            .iter()
            .flat_map(|(k, v)| v.iter().map(|b| (*k, *b)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Provides a reference to the first block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn front(&self) -> Option<(u64, CachedBlock)> {
        self.read().first_key_value().and_then(|(k, v)| v.front().map(|b| (*k, *b)))
    }

    /// Provides a reference to the last block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn back(&self) -> Option<(u64, CachedBlock)> {
        self.read().last_key_value().and_then(|(k, v)| v.back().map(|b| (*k, *b)))
    }

    /// Removes the notification with the given file ID.
    pub(super) fn remove_notification(&self, key: u64) -> Option<VecDeque<CachedBlock>> {
        self.write().remove(&key)
    }

    /// Pops the first block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_front(&self) -> Option<(u64, CachedBlock)> {
        let mut cache = self.write();

        let first_entry = cache.first_entry()?;
        let key = *first_entry.key();
        let blocks = first_entry.into_mut();
        let first_block = blocks.pop_front().unwrap();
        if blocks.is_empty() {
            cache.remove(&key);
        }

        Some((key, first_block))
    }

    /// Pops the last block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_back(&self) -> Option<(u64, CachedBlock)> {
        let mut cache = self.write();

        let last_entry = cache.last_entry()?;
        let key = *last_entry.key();
        let blocks = last_entry.into_mut();
        let last_block = blocks.pop_back().unwrap();
        if blocks.is_empty() {
            cache.remove(&key);
        }

        Some((key, last_block))
    }

    /// Appends a block to the back of the specified file entry.
    pub(super) fn insert(&self, file_id: u64, block: CachedBlock) {
        self.write().entry(file_id).or_default().push_back(block);
    }

    /// Inserts the blocks from the notification into the cache with the given file ID.
    ///
    /// First, inserts the reverted blocks (if any), then the committed blocks (if any).
    pub(super) fn insert_notification_blocks_with_file_id(
        &self,
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
                    },
                );
            }
        }

        if let Some(committed_chain) = committed_chain {
            for block in committed_chain.blocks().values() {
                self.insert(
                    file_id,
                    CachedBlock {
                        action: CachedBlockAction::Commit,
                        block: (block.number, block.hash()).into(),
                    },
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedBlock {
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
