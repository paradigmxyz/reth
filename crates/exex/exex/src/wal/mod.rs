#![allow(dead_code)]

mod cache;
pub use cache::BlockCache;
mod storage;
use eyre::OptionExt;
pub use storage::Storage;

use std::{path::Path, sync::Arc};

use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use reth_exex_types::ExExNotification;
use reth_tracing::tracing::{debug, instrument};

/// WAL is a write-ahead log (WAL) that stores the notifications sent to ExExes.
///
/// WAL is backed by a directory of binary files represented by [`Storage`] and a block cache
/// represented by [`BlockCache`]. The role of the block cache is to avoid walking the WAL directory
/// and decoding notifications every time we want to iterate or finalize the WAL.
///
/// The expected mode of operation is as follows:
/// 1. On every new canonical chain notification, call [`Wal::commit`].
/// 2. When the chain is finalized, call [`Wal::finalize`] to prevent the infinite growth of the
///    WAL.
#[derive(Debug, Clone)]
pub struct Wal {
    inner: Arc<WalInner>,
}

impl Wal {
    /// Creates a new instance of [`Wal`].
    pub fn new(directory: impl AsRef<Path>) -> eyre::Result<Self> {
        Ok(Self { inner: Arc::new(WalInner::new(directory)?) })
    }

    /// Returns a read-only handle to the WAL.
    pub fn handle(&self) -> WalHandle {
        WalHandle { wal: self.inner.clone() }
    }

    /// Commits the notification to WAL.
    pub fn commit(&mut self, notification: &ExExNotification) -> eyre::Result<()> {
        self.inner.commit(notification)
    }

    /// Finalizes the WAL to the given block, inclusive.
    ///
    /// 1. Finds a notification with first unfinalized block (first notification containing a
    ///    committed block higher than `to_block`).
    /// 2. Removes the notifications from the beginning of WAL until the found notification. If this
    ///    notification includes both finalized and non-finalized blocks, it will not be removed.
    pub fn finalize(&self, to_block: BlockNumHash) -> eyre::Result<()> {
        self.inner.finalize(to_block)
    }

    /// Returns an iterator over all notifications in the WAL.
    pub fn iter_notifications(
        &self,
    ) -> eyre::Result<Box<dyn Iterator<Item = eyre::Result<ExExNotification>> + '_>> {
        self.inner.iter_notifications()
    }
}

/// Inner type for the WAL.
#[derive(Debug)]
struct WalInner {
    /// The underlying WAL storage backed by a file.
    storage: Storage,
    /// WAL block cache. See [`cache::BlockCache`] docs for more details.
    block_cache: BlockCache,
}

impl WalInner {
    fn new(directory: impl AsRef<Path>) -> eyre::Result<Self> {
        let mut wal = Self { storage: Storage::new(directory)?, block_cache: BlockCache::new() };
        wal.fill_block_cache()?;
        Ok(wal)
    }

    /// Fills the block cache with the notifications from the storage.
    #[instrument(target = "exex::wal", skip(self))]
    fn fill_block_cache(&mut self) -> eyre::Result<()> {
        let Some(files_range) = self.storage.files_range()? else { return Ok(()) };

        for entry in self.storage.iter_notifications(files_range) {
            let (file_id, notification) = entry?;

            let committed_chain = notification.committed_chain();
            let reverted_chain = notification.reverted_chain();

            debug!(
                target: "exex::wal",
                ?file_id,
                reverted_block_range = ?reverted_chain.as_ref().map(|chain| chain.range()),
                committed_block_range = ?committed_chain.as_ref().map(|chain| chain.range()),
                "Inserting block cache entries"
            );

            self.block_cache.insert_notification_blocks_with_file_id(file_id, &notification);
        }

        Ok(())
    }

    #[instrument(target = "exex::wal", skip_all, fields(
        reverted_block_range = ?notification.reverted_chain().as_ref().map(|chain| chain.range()),
        committed_block_range = ?notification.committed_chain().as_ref().map(|chain| chain.range())
    ))]
    fn commit(&self, notification: &ExExNotification) -> eyre::Result<()> {
        let file_id = self.block_cache.back().map_or(0, |block| block.0 + 1);
        self.storage.write_notification(file_id, notification)?;

        debug!(?file_id, "Inserting notification blocks into the block cache");
        self.block_cache.insert_notification_blocks_with_file_id(file_id, notification);

        Ok(())
    }

    #[instrument(target = "exex::wal", skip(self))]
    fn finalize(&self, to_block: BlockNumHash) -> eyre::Result<()> {
        // First, walk cache to find the file ID of the notification with the finalized block and
        // save the file ID with the first unfinalized block. Do not remove any notifications
        // yet.
        let mut unfinalized_from_file_id = None;
        {
            let mut block_cache = self.block_cache.iter().peekable();
            while let Some((file_id, block)) = block_cache.next() {
                debug!(?file_id, ?block, "Iterating over the block cache");
                if block.action.is_commit() &&
                    block.block.number == to_block.number &&
                    block.block.hash == to_block.hash
                {
                    let notification = self
                        .storage
                        .read_notification(file_id)?
                        .ok_or_eyre("notification not found")?;
                    if notification.committed_chain().unwrap().blocks().len() == 1 {
                        unfinalized_from_file_id = Some(
                            block_cache.peek().map(|(file_id, _)| *file_id).unwrap_or(u64::MAX),
                        );
                    } else {
                        unfinalized_from_file_id = Some(file_id);
                    }

                    debug!(
                        ?file_id,
                        ?block,
                        ?unfinalized_from_file_id,
                        "Found the finalized block in the block cache"
                    );
                    break
                }

                unfinalized_from_file_id = Some(file_id);
            }
        }

        // If the finalized block is still not found, we can't do anything and just return.
        let Some(remove_to_file_id) = unfinalized_from_file_id else {
            debug!("Could not find the finalized block in WAL");
            return Ok(())
        };

        // Remove notifications from the storage from the beginning up to the unfinalized block, not
        // inclusive.
        let (mut file_range_start, mut file_range_end) = (None, None);
        while let Some((file_id, _)) = self.block_cache.front() {
            if file_id == remove_to_file_id {
                break
            }
            self.block_cache.pop_front();

            file_range_start.get_or_insert(file_id);
            file_range_end = Some(file_id);
        }
        debug!(?remove_to_file_id, "Block cache was finalized");

        // Remove notifications from the storage.
        if let Some((file_range_start, file_range_end)) = file_range_start.zip(file_range_end) {
            let removed_notifications =
                self.storage.remove_notifications(file_range_start..=file_range_end)?;
            debug!(?removed_notifications, "Storage was finalized");
        } else {
            debug!("No notifications were finalized from the storage");
        }

        Ok(())
    }

    /// Returns an iterator over all notifications in the WAL.
    fn iter_notifications(
        &self,
    ) -> eyre::Result<Box<dyn Iterator<Item = eyre::Result<ExExNotification>> + '_>> {
        let Some(range) = self.storage.files_range()? else {
            return Ok(Box::new(std::iter::empty()))
        };

        Ok(Box::new(self.storage.iter_notifications(range).map(|entry| Ok(entry?.1))))
    }
}

/// A read-only handle to the WAL that can be shared.
#[derive(Debug)]
pub struct WalHandle {
    wal: Arc<WalInner>,
}

impl WalHandle {
    /// Returns the notification for the given committed block hash if it exists.
    pub fn get_committed_notification_by_block_hash(
        &self,
        block_hash: &B256,
    ) -> eyre::Result<Option<ExExNotification>> {
        let Some(file_id) = self.wal.block_cache.get_file_id_by_committed_block_hash(block_hash)
        else {
            return Ok(None)
        };

        self.wal.storage.read_notification(file_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eyre::OptionExt;
    use reth_exex_types::ExExNotification;
    use reth_provider::Chain;
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, BlockParams, BlockRangeParams,
    };

    use crate::wal::{
        cache::{CachedBlock, CachedBlockAction},
        Wal,
    };

    fn read_notifications(wal: &Wal) -> eyre::Result<Vec<ExExNotification>> {
        let Some(files_range) = wal.inner.storage.files_range()? else { return Ok(Vec::new()) };

        wal.inner
            .storage
            .iter_notifications(files_range)
            .map(|entry| Ok(entry?.1))
            .collect::<eyre::Result<_>>()
    }

    #[test]
    fn test_wal() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut rng = generators::rng();

        // Create an instance of the WAL in a temporary directory
        let temp_dir = tempfile::tempdir()?;
        let mut wal = Wal::new(&temp_dir)?;
        assert!(wal.inner.block_cache.is_empty());

        // Create 4 canonical blocks and one reorged block with number 2
        let blocks = random_block_range(&mut rng, 0..=3, BlockRangeParams::default())
            .into_iter()
            .map(|block| block.seal_with_senders().ok_or_eyre("failed to recover senders"))
            .collect::<eyre::Result<Vec<_>>>()?;
        let block_1_reorged = random_block(
            &mut rng,
            1,
            BlockParams { parent: Some(blocks[0].hash()), ..Default::default() },
        )
        .seal_with_senders()
        .ok_or_eyre("failed to recover senders")?;
        let block_2_reorged = random_block(
            &mut rng,
            2,
            BlockParams { parent: Some(blocks[1].hash()), ..Default::default() },
        )
        .seal_with_senders()
        .ok_or_eyre("failed to recover senders")?;

        // Create notifications for the above blocks.
        // 1. Committed notification for blocks with number 0 and 1
        // 2. Reverted notification for block with number 1
        // 3. Committed notification for block with number 1 and 2
        // 4. Reorged notification for block with number 2 that was reverted, and blocks with number
        //    2 and 3 that were committed
        let committed_notification_1 = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![blocks[0].clone(), blocks[1].clone()],
                Default::default(),
                None,
            )),
        };
        let reverted_notification = ExExNotification::ChainReverted {
            old: Arc::new(Chain::new(vec![blocks[1].clone()], Default::default(), None)),
        };
        let committed_notification_2 = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![block_1_reorged.clone(), blocks[2].clone()],
                Default::default(),
                None,
            )),
        };
        let reorged_notification = ExExNotification::ChainReorged {
            old: Arc::new(Chain::new(vec![blocks[2].clone()], Default::default(), None)),
            new: Arc::new(Chain::new(
                vec![block_2_reorged.clone(), blocks[3].clone()],
                Default::default(),
                None,
            )),
        };

        // Commit notifications, verify that the block cache is updated and the notifications are
        // written to WAL.

        // First notification (commit block 0, 1)
        let file_id = 0;
        let committed_notification_1_cache = vec![
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (blocks[0].number, blocks[0].hash()).into(),
                    parent_hash: blocks[0].parent_hash,
                },
            ),
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (blocks[1].number, blocks[1].hash()).into(),
                    parent_hash: blocks[1].parent_hash,
                },
            ),
        ];
        wal.commit(&committed_notification_1)?;
        assert_eq!(
            wal.inner.block_cache.iter().collect::<Vec<_>>(),
            committed_notification_1_cache
        );
        assert_eq!(read_notifications(&wal)?, vec![committed_notification_1.clone()]);

        // Second notification (revert block 1)
        wal.commit(&reverted_notification)?;
        let file_id = 1;
        let reverted_notification_cache = vec![(
            file_id,
            CachedBlock {
                action: CachedBlockAction::Revert,
                block: (blocks[1].number, blocks[1].hash()).into(),
                parent_hash: blocks[1].parent_hash,
            },
        )];
        assert_eq!(
            wal.inner.block_cache.iter().collect::<Vec<_>>(),
            [committed_notification_1_cache.clone(), reverted_notification_cache.clone()].concat()
        );
        assert_eq!(
            read_notifications(&wal)?,
            vec![committed_notification_1.clone(), reverted_notification.clone()]
        );

        // Third notification (commit block 1, 2)
        wal.commit(&committed_notification_2)?;
        let file_id = 2;
        let committed_notification_2_cache = vec![
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (block_1_reorged.number, block_1_reorged.hash()).into(),
                    parent_hash: block_1_reorged.parent_hash,
                },
            ),
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (blocks[2].number, blocks[2].hash()).into(),
                    parent_hash: blocks[2].parent_hash,
                },
            ),
        ];
        assert_eq!(
            wal.inner.block_cache.iter().collect::<Vec<_>>(),
            [
                committed_notification_1_cache.clone(),
                reverted_notification_cache.clone(),
                committed_notification_2_cache.clone()
            ]
            .concat()
        );
        assert_eq!(
            read_notifications(&wal)?,
            vec![
                committed_notification_1.clone(),
                reverted_notification.clone(),
                committed_notification_2.clone()
            ]
        );

        // Fourth notification (revert block 2, commit block 2, 3)
        wal.commit(&reorged_notification)?;
        let file_id = 3;
        let reorged_notification_cache = vec![
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Revert,
                    block: (blocks[2].number, blocks[2].hash()).into(),
                    parent_hash: blocks[2].parent_hash,
                },
            ),
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (block_2_reorged.number, block_2_reorged.hash()).into(),
                    parent_hash: block_2_reorged.parent_hash,
                },
            ),
            (
                file_id,
                CachedBlock {
                    action: CachedBlockAction::Commit,
                    block: (blocks[3].number, blocks[3].hash()).into(),
                    parent_hash: blocks[3].parent_hash,
                },
            ),
        ];
        assert_eq!(
            wal.inner.block_cache.iter().collect::<Vec<_>>(),
            [
                committed_notification_1_cache,
                reverted_notification_cache,
                committed_notification_2_cache.clone(),
                reorged_notification_cache.clone()
            ]
            .concat()
        );
        assert_eq!(
            read_notifications(&wal)?,
            vec![
                committed_notification_1.clone(),
                reverted_notification.clone(),
                committed_notification_2.clone(),
                reorged_notification.clone()
            ]
        );

        // Now, finalize the WAL up to the block 1. Block 1 was in the third notification that also
        // had block 2 committed. In this case, we can't split the notification into two parts, so
        // we preserve the whole notification in both the block cache and the storage, and delete
        // the notifications before it.
        wal.finalize((block_1_reorged.number, block_1_reorged.hash()).into())?;
        assert_eq!(
            wal.inner.block_cache.iter().collect::<Vec<_>>(),
            [committed_notification_2_cache, reorged_notification_cache].concat()
        );
        assert_eq!(read_notifications(&wal)?, vec![committed_notification_2, reorged_notification]);

        Ok(())
    }
}
