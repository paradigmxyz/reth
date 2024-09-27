use std::collections::{BTreeMap, VecDeque};

use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use dashmap::DashMap;
use parking_lot::RwLock;
use reth_exex_types::ExExNotification;

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
    files: RwLock<BTreeMap<u64, VecDeque<CachedBlock>>>,
    /// A mapping of committed blocks `Block Hash -> Block`.
    ///
    /// For each [`ExExNotification::ChainCommitted`] notification, there will be an entry per
    /// block.
    committed_blocks: DashMap<B256, (u64, CachedBlock)>,
}

impl BlockCache {
    /// Creates a new instance of [`BlockCache`].
    pub(super) fn new() -> Self {
        Self { files: RwLock::new(BTreeMap::new()), committed_blocks: DashMap::new() }
    }

    /// Returns `true` if the cache is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.files.read().is_empty()
    }

    /// Returns a front-to-back iterator.
    pub(super) fn iter(&self) -> impl Iterator<Item = (u64, CachedBlock)> + '_ {
        self.files
            .read()
            .iter()
            .flat_map(|(k, v)| v.iter().map(move |b| (*k, *b)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Provides a reference to the first block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn front(&self) -> Option<(u64, CachedBlock)> {
        self.files.read().first_key_value().and_then(|(k, v)| v.front().map(|b| (*k, *b)))
    }

    /// Provides a reference to the last block from the cache, or `None` if the cache is
    /// empty.
    pub(super) fn back(&self) -> Option<(u64, CachedBlock)> {
        self.files.read().last_key_value().and_then(|(k, v)| v.back().map(|b| (*k, *b)))
    }

    /// Removes the notification with the given file ID.
    pub(super) fn remove_notification(&self, key: u64) -> Option<VecDeque<CachedBlock>> {
        self.files.write().remove(&key)
    }

    /// Pops the first block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_front(&self) -> Option<(u64, CachedBlock)> {
        let mut files = self.files.write();

        let first_entry = files.first_entry()?;
        let key = *first_entry.key();
        let blocks = first_entry.into_mut();
        let first_block = blocks.pop_front().unwrap();
        if blocks.is_empty() {
            files.remove(&key);
        }

        Some((key, first_block))
    }

    /// Pops the last block from the cache. If it resulted in the whole file entry being empty,
    /// it will also remove the file entry.
    pub(super) fn pop_back(&self) -> Option<(u64, CachedBlock)> {
        let mut files = self.files.write();

        let last_entry = files.last_entry()?;
        let key = *last_entry.key();
        let blocks = last_entry.into_mut();
        let last_block = blocks.pop_back().unwrap();
        if blocks.is_empty() {
            files.remove(&key);
        }

        Some((key, last_block))
    }

    /// Returns the file ID for the notification containing the given committed block hash, if it
    /// exists.
    pub(super) fn get_file_id_by_committed_block_hash(&self, block_hash: &B256) -> Option<u64> {
        self.committed_blocks.get(block_hash).map(|entry| entry.0)
    }

    /// Inserts the blocks from the notification into the cache with the given file ID.
    ///
    /// First, inserts the reverted blocks (if any), then the committed blocks (if any).
    pub(super) fn insert_notification_blocks_with_file_id(
        &self,
        file_id: u64,
        notification: &ExExNotification,
    ) {
        let mut files = self.files.write();

        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        if let Some(reverted_chain) = reverted_chain {
            for block in reverted_chain.blocks().values() {
                files.entry(file_id).or_default().push_back(CachedBlock {
                    action: CachedBlockAction::Revert,
                    block: (block.number, block.hash()).into(),
                    parent_hash: block.parent_hash,
                });
            }
        }

        if let Some(committed_chain) = committed_chain {
            for block in committed_chain.blocks().values() {
                let cached_block = CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (block.number, block.hash()).into(),
                    parent_hash: block.parent_hash,
                };
                files.entry(file_id).or_default().push_back(cached_block);
                self.committed_blocks.insert(block.hash(), (file_id, cached_block));
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
