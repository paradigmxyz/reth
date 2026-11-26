//! Stage and pruning checkpoints, chain markers, and database metadata.

use super::DatabaseProvider;
use crate::{
    ChainStateBlockReader, ChainStateBlockWriter, EitherWriter, PersistenceFrontiers,
    ProviderError, PruneCheckpointReader, PruneCheckpointWriter, StageCheckpointReader,
    StageCheckpointWriter,
};
use alloy_consensus::BlockHeader;
use alloy_primitives::BlockNumber;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_types::NodeTypes;
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{MetadataProvider, MetadataWriter, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use tracing::instrument;

impl<TX: DbTx, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Rejects advances of either the Finish block number or its state/trie frontier while a
    /// persisted snap attempt is unverified. A missing partial frontier equals the block
    /// number, so clearing it can advance state even when the block number stays put or rewinds.
    /// Updates that advance neither frontier are allowed.
    fn ensure_finish_may_advance(&self, proposed: &StageCheckpoint) -> ProviderResult<()> {
        let current = self.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default();
        let state_trie_frontier = |checkpoint: &StageCheckpoint| {
            checkpoint
                .finish_stage_checkpoint()
                .and_then(|finish| finish.partial_state_trie())
                .unwrap_or(checkpoint.block_number)
        };
        if proposed.block_number <= current.block_number &&
            state_trie_frontier(proposed) <= state_trie_frontier(&current)
        {
            return Ok(())
        }
        match self.snap_attempt()? {
            Some(attempt) if !attempt.is_verified() => {
                Err(ProviderError::UnverifiedSnapState { attempt: attempt.id().into() })
            }
            _ => Ok(()),
        }
    }

    /// Refuses snap sync on a database without the hashed state layout it downloads into.
    pub fn ensure_snap_sync_layout(&self) -> ProviderResult<()> {
        if self.cached_storage_settings().use_hashed_state() {
            Ok(())
        } else {
            Err(ProviderError::SnapStorageLayoutUnsupported)
        }
    }

    /// Returns whether snap sync bootstraps this database: an attempt is left to finish or
    /// replace, or nothing has executed past genesis.
    ///
    /// Call this after genesis is initialized: until then the cached settings fall back to the
    /// legacy layout and no execution checkpoint exists.
    pub fn snap_bootstraps(&self) -> ProviderResult<bool> {
        if let Some(attempt) = self.snap_attempt()? {
            return Ok(!attempt.is_verified())
        }
        let genesis = self.chain_spec().genesis_header().number();
        Ok(self
            .get_stage_checkpoint(StageId::Execution)?
            .is_none_or(|checkpoint| checkpoint.block_number == genesis))
    }

    /// Refuses a database the selected sync can't continue: snap needs the hashed state layout
    /// when it bootstraps, and only snap can finish or replace an unverified snap attempt.
    ///
    /// Call this after genesis is initialized, see [`Self::snap_bootstraps`].
    pub fn ensure_sync_mode(&self, snap_enabled: bool) -> ProviderResult<()> {
        if snap_enabled {
            return if self.snap_bootstraps()? { self.ensure_snap_sync_layout() } else { Ok(()) }
        }
        match self.snap_attempt()? {
            Some(attempt) if !attempt.is_verified() => {
                Err(ProviderError::SnapStateRequiresSnapSync { attempt: attempt.id().into() })
            }
            _ => Ok(()),
        }
    }
}

impl<TX: DbTx, N: NodeTypes> StageCheckpointReader for DatabaseProvider<TX, N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(if let Some(encoded) = id.get_pre_encoded() {
            self.tx.get_by_encoded_key::<tables::StageCheckpoints>(encoded)?
        } else {
            self.tx.get::<tables::StageCheckpoints>(id.to_string())?
        })
    }

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(self.tx.get::<tables::StageCheckpointProgresses>(id.to_string())?)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.tx
            .cursor_read::<tables::StageCheckpoints>()?
            .walk(None)?
            .collect::<Result<Vec<(String, StageCheckpoint)>, _>>()
            .map_err(ProviderError::Database)
    }
}

impl<TX: DbTxMut + DbTx, N: NodeTypes> StageCheckpointWriter for DatabaseProvider<TX, N> {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(
        &self,
        id: StageId,
        checkpoint: StageCheckpoint,
    ) -> ProviderResult<()> {
        if id.is_finish() {
            self.ensure_finish_may_advance(&checkpoint)?;
        }
        Ok(self.tx.put::<tables::StageCheckpoints>(id.to_string(), checkpoint)?)
    }

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(
        &self,
        id: StageId,
        checkpoint: Vec<u8>,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StageCheckpointProgresses>(id.to_string(), checkpoint)?)
    }

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> ProviderResult<()> {
        let current = self.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default();
        let proposed = StageCheckpoint {
            block_number,
            ..if drop_stage_checkpoint { Default::default() } else { current }
        };
        self.ensure_finish_may_advance(&proposed)?;

        // iterate over all existing stages in the table and update its progress.
        let mut cursor = self.tx.cursor_write::<tables::StageCheckpoints>()?;
        for stage_id in StageId::ALL {
            let (_, checkpoint) = cursor.seek_exact(stage_id.to_string())?.unwrap_or_default();
            cursor.upsert(
                stage_id.to_string(),
                &StageCheckpoint {
                    block_number,
                    ..if drop_stage_checkpoint { Default::default() } else { checkpoint }
                },
            )?;
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Updates pipeline checkpoints after an unwind while preserving an explicitly lagging
    /// state/trie frontier.
    pub(super) fn update_pipeline_stages_after_unwind(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<PersistenceFrontiers> {
        let partial_state_trie = self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| {
                checkpoint
                    .finish_stage_checkpoint()
                    .and_then(|finish| finish.partial_state_trie())
                    .unwrap_or(checkpoint.block_number)
            })
            .unwrap_or(block_number)
            .min(block_number);

        self.update_pipeline_stages(block_number, true)?;
        if partial_state_trie < block_number {
            self.save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(block_number).with_finish_stage_checkpoint(FinishCheckpoint {
                    partial_state_trie: Some(partial_state_trie),
                }),
            )?;
        }

        Ok(PersistenceFrontiers { db_tip: block_number, partial_state_trie })
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> PruneCheckpointReader for DatabaseProvider<TX, N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        Ok(self.tx.get::<tables::PruneCheckpoints>(segment)?)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        Ok(PruneSegment::variants()
            .filter_map(|segment| {
                self.tx
                    .get::<tables::PruneCheckpoints>(segment)
                    .transpose()
                    .map(|chk| chk.map(|chk| (segment, chk)))
            })
            .collect::<Result<_, _>>()?)
    }

    fn get_prune_checkpoints_for_availability(
        &self,
    ) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        let mut checkpoints = self.get_prune_checkpoints()?;
        if !EitherWriter::receipts_destination(self).is_static_file() ||
            !checkpoints.iter().any(|(segment, checkpoint)| {
                matches!(segment, PruneSegment::Receipts | PruneSegment::ContractLogs) &&
                    checkpoint.tx_number.is_some()
            })
        {
            return Ok(checkpoints)
        }

        let Some(lowest_block) =
            self.static_file_provider.get_lowest_range_start(StaticFileSegment::Receipts)
        else {
            return Ok(checkpoints)
        };
        let Some(first_tx_range) = self
            .static_file_provider
            .get_segment_provider_for_block(StaticFileSegment::Receipts, lowest_block, None)?
            .user_header()
            .tx_range()
        else {
            return Ok(checkpoints)
        };

        // Migration can pad the first file with blocks whose receipts were already pruned.
        // TransactionBlocks::seek can also land after expired history, so verify the body range.
        let pruned_tx = first_tx_range.start().checked_sub(1);
        let pruned_block = if let Some(tx) = pruned_tx {
            if let Some((_, block)) =
                self.tx.cursor_read::<tables::TransactionBlocks>()?.seek(tx)? &&
                let Some(indices) = self.tx.get::<tables::BlockBodyIndices>(block)? &&
                indices.tx_num_range().contains(&tx)
            {
                Some(block).max(lowest_block.checked_sub(1))
            } else {
                return Ok(checkpoints)
            }
        } else {
            lowest_block.checked_sub(1)
        };

        // The removed StaticFileReceipts pruner checkpointed moves into static files.
        // Writers enforce continuity within each file, but migration can leave gaps between files.
        // Reuse the verified coverage for both receipts and logs.
        let mut last_tx = first_tx_range.end();
        for (segment, checkpoint) in &mut checkpoints {
            if matches!(segment, PruneSegment::Receipts | PruneSegment::ContractLogs) &&
                let Some(tx) = checkpoint.tx_number &&
                tx >= first_tx_range.start()
            {
                while let Some(next_tx) = last_tx.checked_add(1) &&
                    next_tx <= tx &&
                    let Some(next_file) = self
                        .static_file_provider
                        .get_maybe_segment_provider(StaticFileSegment::Receipts, next_tx)? &&
                    let Some(next_range) = next_file.user_header().tx_range() &&
                    next_range.start() == next_tx
                {
                    last_tx = next_range.end();
                }
                if tx <= last_tx {
                    checkpoint.block_number = pruned_block;
                    checkpoint.tx_number = pruned_tx;
                }
            }
        }

        Ok(checkpoints)
    }
}

impl<TX: DbTxMut, N: NodeTypes> PruneCheckpointWriter for DatabaseProvider<TX, N> {
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::PruneCheckpoints>(segment, checkpoint)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> ChainStateBlockReader for DatabaseProvider<TX, N> {
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock)?)
    }

    fn last_safe_block_number(&self) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::ChainState>(tables::ChainStateKey::LastSafeBlock)?)
    }
}

impl<TX: DbTxMut, N: NodeTypes> ChainStateBlockWriter for DatabaseProvider<TX, N> {
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        Ok(self
            .tx
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, block_number)?)
    }

    fn save_safe_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::ChainState>(tables::ChainStateKey::LastSafeBlock, block_number)?)
    }
}

impl<TX: DbTx, N: NodeTypes> MetadataProvider for DatabaseProvider<TX, N> {
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>> {
        self.tx.get::<tables::Metadata>(key.to_string()).map_err(Into::into)
    }
}

impl<TX: DbTxMut, N: NodeTypes> MetadataWriter for DatabaseProvider<TX, N> {
    fn write_metadata(&self, key: &str, value: Vec<u8>) -> ProviderResult<()> {
        self.tx.put::<tables::Metadata>(key.to_string(), value).map_err(Into::into)
    }

    fn delete_metadata(&self, key: &str) -> ProviderResult<()> {
        self.tx.delete::<tables::Metadata>(key.to_string(), None)?;
        Ok(())
    }
}
