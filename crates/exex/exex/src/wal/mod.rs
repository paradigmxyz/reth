#![allow(dead_code)]

mod cache;
mod storage;

use std::{ops::ControlFlow, path::Path};

use cache::BlockCache;
use eyre::OptionExt;
use reth_exex_types::ExExNotification;
use reth_primitives::BlockNumHash;
use reth_tracing::tracing::{debug, instrument};
use storage::Storage;

/// The maximum number of blocks to cache.
///
/// [`CachedBlock`] has a size of `u64 + u64 + B256` which is 384 bits. 384 bits * 1 million = 48
/// megabytes.
const MAX_CACHED_BLOCKS: usize = 1_000_000;

#[derive(Debug)]
pub(crate) struct Wal {
    /// The underlying WAL storage backed by a file.
    storage: Storage,
    /// The block cache of the WAL. Acts as a FIFO queue with a maximum size of
    /// [`MAX_CACHED_BLOCKS`].
    ///
    /// For each notification written to the WAL, there will be an entry per block written to
    /// the cache with the same file offset as the notification in the [`Storage`]. I.e. for each
    /// notification, there may be multiple blocks in the cache.
    ///
    /// This cache is needed only for convenience, so we can avoid walking the [`Storage`] every
    /// time we want to find a notification corresponding to a block.
    block_cache: BlockCache,
}

impl Wal {
    /// Creates a new instance of [`Wal`].
    pub(crate) fn new(directory: impl AsRef<Path>) -> eyre::Result<Self> {
        let path = directory.as_ref().join("latest.wal");

        let mut wal =
            Self { storage: Storage::new(path)?, block_cache: BlockCache::new(MAX_CACHED_BLOCKS) };
        wal.fill_block_cache(u64::MAX)?;

        Ok(wal)
    }

    /// Clears the block cache and fills it with the notifications from the [`Storage`], up to the
    /// given offset in bytes, not inclusive.
    #[instrument(target = "exex::wal", skip(self))]
    fn fill_block_cache(&mut self, to_offset: u64) -> eyre::Result<()> {
        self.block_cache.clear();

        let mut file_offset = 0;
        self.storage.for_each_notification(|raw_len, notification| {
            let committed_chain = notification.committed_chain();
            let reverted_chain = notification.reverted_chain();

            debug!(
                target: "exex::wal",
                ?file_offset,
                reverted_block_range = ?reverted_chain.as_ref().map(|chain| chain.range()),
                committed_block_range = ?committed_chain.as_ref().map(|chain| chain.range()),
                "Inserting block cache entries"
            );

            self.block_cache.insert_notification_blocks_with_offset(&notification, file_offset);

            if file_offset >= to_offset {
                debug!(
                    target: "exex::wal",
                    ?file_offset,
                    "Reached the requested offset when filling the block cache"
                );
                return Ok(ControlFlow::Break(()))
            }

            file_offset += raw_len as u64;

            Ok(ControlFlow::Continue(()))
        })?;

        Ok(())
    }

    /// Commits the notification to WAL.
    #[instrument(target = "exex::wal", skip_all)]
    pub(crate) fn commit(&mut self, notification: &ExExNotification) -> eyre::Result<()> {
        let reverted_chain = notification.reverted_chain();
        let committed_chain = notification.committed_chain();

        let file_offset = self.storage.bytes_len()?;

        debug!(
            reverted_block_range = ?reverted_chain.as_ref().map(|chain| chain.range()),
            committed_block_range = ?committed_chain.as_ref().map(|chain| chain.range()),
            ?file_offset,
            "Writing notification to WAL"
        );
        self.storage.write_notification(notification)?;

        debug!(
            reverted_block_range = ?reverted_chain.as_ref().map(|chain| chain.range()),
            committed_block_range = ?committed_chain.as_ref().map(|chain| chain.range()),
            ?file_offset,
            "Inserting notification blocks into the block cache"
        );
        self.block_cache.insert_notification_blocks_with_offset(notification, file_offset);

        Ok(())
    }

    /// Rollbacks the WAL to the given block, inclusive.
    ///
    /// 1. Walks the WAL from the end and searches for the first notification where committed chain
    ///    contains a block with the same number and hash as `to_block`.
    /// 2. If the notification is found, truncates the WAL to the offset of the notification. It
    ///    means that if the notification contains both given block and blocks before it, the whole
    ///    notification will be truncated.
    ///
    /// # Returns
    ///
    /// The block number and hash of the lowest removed block. The caller is expected to backfill
    /// the blocks between the returned block and the given `to_block`, if there's any.
    #[instrument(target = "exex::wal", skip(self))]
    pub(crate) fn rollback(
        &mut self,
        to_block: BlockNumHash,
    ) -> eyre::Result<Option<BlockNumHash>> {
        let mut truncate_to = None;
        let mut lowest_removed_block = None;
        loop {
            let Some(block) = self.block_cache.pop_back() else {
                debug!(
                    ?truncate_to,
                    ?lowest_removed_block,
                    "No blocks in the block cache, filling the block cache"
                );
                self.fill_block_cache(truncate_to.unwrap_or(u64::MAX))?;
                if self.block_cache.is_empty() {
                    debug!(
                        ?truncate_to,
                        ?lowest_removed_block,
                        "No blocks in the block cache, and filling the block cache didn't change anything"
                    );
                    break
                }
                continue
            };

            if block.action.is_commit() && block.block.number == to_block.number {
                debug!(?truncate_to, ?lowest_removed_block, "Found the requested block");

                if block.block.hash != to_block.hash {
                    eyre::bail!("block hash mismatch in WAL")
                }

                truncate_to = Some(block.file_offset);

                let notification = self
                    .storage
                    .read_notification_at(block.file_offset)?
                    .ok_or_eyre("failed to read notification at offset")?;
                lowest_removed_block = notification
                    .committed_chain()
                    .as_ref()
                    .map(|chain| chain.first())
                    .map(|block| (block.number, block.hash()).into());
                break
            }

            truncate_to = Some(block.file_offset);
        }

        if let Some(truncate_to) = truncate_to {
            self.storage.truncate_to_offset(truncate_to)?;
            debug!(?truncate_to, "Truncated the storage");
        } else {
            debug!("No blocks were truncated. Block cache was filled.");
        }
        self.fill_block_cache(u64::MAX)?;

        Ok(lowest_removed_block)
    }

    /// Finalizes the WAL to the given block, inclusive.
    ///
    /// 1. Finds an offset of the notification with first unfinalized block (first notification
    ///    containing a committed block higher than `to_block`). If the notificatin includes both
    ///    finalized and non-finalized blocks, the offset will include this notification (i.e.
    ///    consider this notification unfinalized and don't remove it).
    /// 2. Truncates the storage from the offset of the notification, not inclusive.
    #[instrument(target = "exex::wal", skip(self))]
    pub(crate) fn finalize(&mut self, to_block: BlockNumHash) -> eyre::Result<()> {
        // First, walk cache to find the offset of the notification with the finalized block.
        let mut unfinalized_from_offset = None;
        while let Some(cached_block) = self.block_cache.pop_front() {
            if cached_block.action.is_commit() &&
                cached_block.block.number == to_block.number &&
                cached_block.block.hash == to_block.hash
            {
                unfinalized_from_offset = Some(
                    self.block_cache
                        .front()
                        .map_or_else(|| self.storage.bytes_len(), |block| Ok(block.file_offset))?,
                );

                debug!(?unfinalized_from_offset, "Found the finalized block in the block cache");
                break
            }
        }

        // If the finalized block is not found in cache, we need to walk the whole storage.
        if unfinalized_from_offset.is_none() {
            debug!("Finalized block not found in the block cache, walking the whole storage");

            let mut file_offset = 0;
            self.storage.for_each_notification(|raw_data_len, notification| {
                if let Some(committed_chain) = notification.committed_chain() {
                    if let Some(finalized_block) = committed_chain.blocks().get(&to_block.number) {
                        if finalized_block.hash() == to_block.hash {
                            if committed_chain.blocks().len() == 1 {
                                // If the committed chain only contains the finalized block, we can
                                // truncate the WAL including the notification itself.
                                unfinalized_from_offset = Some(file_offset + raw_data_len as u64);
                            } else {
                                // Otherwise, we need to truncate the WAL to the offset of the
                                // notification and leave it in the WAL.
                                unfinalized_from_offset = Some(file_offset);
                            }

                            debug!(
                                ?unfinalized_from_offset,
                                committed_block_range = ?committed_chain.range(),
                                "Found the finalized block in the storage"
                            );
                            return Ok(ControlFlow::Break(()))
                        }
                    }
                }

                file_offset += raw_data_len as u64;

                Ok(ControlFlow::Continue(()))
            })?;
        }

        // If the finalized block is still not found, we can't do anything and just return.
        let Some(unfinalized_from_offset) = unfinalized_from_offset else {
            debug!("Could not find the finalized block in WAL");
            return Ok(())
        };

        // Truncate the storage to the unfinalized block.
        let (old_size, new_size) = self.storage.truncate_from_offset(unfinalized_from_offset)?;

        // Fill the block cache with the notifications from updated storage.
        self.fill_block_cache(u64::MAX)?;

        debug!(?old_size, ?new_size, "WAL was finalized and block cache was filled");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::ControlFlow, sync::Arc};

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

    fn read_notifications(wal: &mut Wal) -> eyre::Result<Vec<ExExNotification>> {
        let mut notifications = Vec::new();
        wal.storage.for_each_notification(|_, notification| {
            notifications.push(notification);
            Ok(ControlFlow::Continue(()))
        })?;
        Ok(notifications)
    }

    #[test]
    fn test_wal() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut rng = generators::rng();

        // Create an instance of the WAL in a temporary directory
        let temp_dir = tempfile::tempdir()?;
        let mut wal = Wal::new(&temp_dir)?;
        assert!(wal.block_cache.is_empty());

        // Create 4 canonical blocks and one reorged block with number 2
        let blocks = random_block_range(&mut rng, 0..=3, BlockRangeParams::default())
            .into_iter()
            .map(|block| block.seal_with_senders().ok_or_eyre("failed to recover senders"))
            .collect::<eyre::Result<Vec<_>>>()?;
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
                vec![blocks[1].clone(), blocks[2].clone()],
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
        let file_offset = wal.storage.bytes_len()?;
        let committed_notification_1_cache = vec![
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (blocks[0].number, blocks[0].hash()).into(),
            },
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (blocks[1].number, blocks[1].hash()).into(),
            },
        ];
        wal.commit(&committed_notification_1)?;
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [committed_notification_1_cache.clone()].concat()
        );
        assert_eq!(read_notifications(&mut wal)?, vec![committed_notification_1.clone()]);

        // Second notification (revert block 1)
        let file_offset = wal.storage.bytes_len()?;
        wal.commit(&reverted_notification)?;
        let reverted_notification_cache = vec![CachedBlock {
            file_offset,
            action: CachedBlockAction::Revert,
            block: (blocks[1].number, blocks[1].hash()).into(),
        }];
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [committed_notification_1_cache.clone(), reverted_notification_cache.clone()].concat()
        );
        assert_eq!(
            read_notifications(&mut wal)?,
            vec![committed_notification_1.clone(), reverted_notification.clone()]
        );

        // Now, rollback to block 1 and verify that both the block cache and the storage are
        // empty. We expect the rollback to delete the first notification (commit block 0, 1),
        // because we can't delete blocks partly from the notification. Additionally, check that
        // the block that the rolled back to is the block with number 0.
        let rolled_back_to = wal.rollback((blocks[1].number, blocks[1].hash()).into())?;
        assert_eq!(wal.block_cache.iter().copied().collect::<Vec<_>>(), vec![]);
        assert_eq!(wal.storage.bytes_len()?, 0);
        assert_eq!(rolled_back_to, Some((blocks[0].number, blocks[0].hash()).into()));

        // Commit notifications 1 and 2 again
        wal.commit(&committed_notification_1)?;
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [committed_notification_1_cache.clone()].concat()
        );
        assert_eq!(read_notifications(&mut wal)?, vec![committed_notification_1.clone()]);
        wal.commit(&reverted_notification)?;
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [committed_notification_1_cache.clone(), reverted_notification_cache.clone()].concat()
        );
        assert_eq!(
            read_notifications(&mut wal)?,
            vec![committed_notification_1.clone(), reverted_notification.clone()]
        );

        // Third notification (commit block 1, 2)
        let file_offset = wal.storage.bytes_len()?;
        wal.commit(&committed_notification_2)?;
        let committed_notification_2_cache = vec![
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (blocks[1].number, blocks[1].hash()).into(),
            },
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (blocks[2].number, blocks[2].hash()).into(),
            },
        ];
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [
                committed_notification_1_cache.clone(),
                reverted_notification_cache.clone(),
                committed_notification_2_cache.clone()
            ]
            .concat()
        );
        assert_eq!(
            read_notifications(&mut wal)?,
            vec![
                committed_notification_1.clone(),
                reverted_notification.clone(),
                committed_notification_2.clone()
            ]
        );

        // Fourth notification (revert block 2, commit block 2, 3)
        let file_offset = wal.storage.bytes_len()?;
        wal.commit(&reorged_notification)?;
        let reorged_notification_cache = vec![
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Revert,
                block: (blocks[2].number, blocks[2].hash()).into(),
            },
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (block_2_reorged.number, block_2_reorged.hash()).into(),
            },
            CachedBlock {
                file_offset,
                action: CachedBlockAction::Commit,
                block: (blocks[3].number, blocks[3].hash()).into(),
            },
        ];
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            [
                committed_notification_1_cache,
                reverted_notification_cache.clone(),
                committed_notification_2_cache.clone(),
                reorged_notification_cache.clone()
            ]
            .concat()
        );
        assert_eq!(
            read_notifications(&mut wal)?,
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
        wal.finalize((blocks[1].number, blocks[1].hash()).into())?;
        let finalized_bytes_amount = reverted_notification_cache[0].file_offset;
        let finalized_notification_cache = [
            reverted_notification_cache,
            committed_notification_2_cache,
            reorged_notification_cache,
        ]
        .concat()
        .into_iter()
        .map(|block| CachedBlock {
            file_offset: block.file_offset - finalized_bytes_amount,
            ..block
        })
        .collect::<Vec<_>>();
        assert_eq!(
            wal.block_cache.iter().copied().collect::<Vec<_>>(),
            finalized_notification_cache
        );
        assert_eq!(
            read_notifications(&mut wal)?,
            vec![reverted_notification, committed_notification_2, reorged_notification]
        );

        Ok(())
    }
}
