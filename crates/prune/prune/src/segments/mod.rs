mod account_history;
mod headers;
pub(super) mod history;
mod receipts;
mod receipts_by_logs;
mod sender_recovery;
mod set;
mod storage_history;
mod transaction_lookup;
mod transactions;

use crate::PrunerError;
pub use account_history::AccountHistory;
use alloy_primitives::{BlockNumber, TxNumber};
pub use headers::Headers;
pub use receipts::Receipts;
pub use receipts_by_logs::ReceiptsByLogs;
use reth_db_api::database::Database;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DatabaseProviderRW, PruneCheckpointWriter,
};
use reth_prune_types::{
    PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress, PruneSegment,
};
pub use sender_recovery::SenderRecovery;
pub use set::SegmentSet;
use std::{fmt::Debug, ops::RangeInclusive};
pub use storage_history::StorageHistory;
use tracing::error;
pub use transaction_lookup::TransactionLookup;
pub use transactions::Transactions;

/// A segment represents a pruning of some portion of the data.
///
/// Segments are called from [Pruner](crate::Pruner) with the following lifecycle:
/// 1. Call [`Segment::prune`] with `delete_limit` of [`PruneInput`].
/// 2. If [`Segment::prune`] returned a [Some] in `checkpoint` of [`PruneOutput`], call
///    [`Segment::save_checkpoint`].
/// 3. Subtract `pruned` of [`PruneOutput`] from `delete_limit` of next [`PruneInput`].
pub trait Segment<DB: Database>: Debug + Send + Sync {
    /// Segment of data that's pruned.
    fn segment(&self) -> PruneSegment;

    /// Prune mode with which the segment was initialized
    fn mode(&self) -> Option<PruneMode>;

    /// Prune data for [`Self::segment`] using the provided input.
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError>;

    /// Save checkpoint for [`Self::segment`] to the database.
    fn save_checkpoint(
        &self,
        provider: &DatabaseProviderRW<DB>,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        provider.save_prune_checkpoint(self.segment(), checkpoint)
    }
}

/// Segment pruning input, see [`Segment::prune`].
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct PruneInput {
    pub(crate) previous_checkpoint: Option<PruneCheckpoint>,
    /// Target block up to which the pruning needs to be done, inclusive.
    pub(crate) to_block: BlockNumber,
    /// Limits pruning of a segment.
    pub(crate) limiter: PruneLimiter,
}

impl PruneInput {
    /// Get next inclusive tx number range to prune according to the checkpoint and `to_block` block
    /// number.
    ///
    /// To get the range start:
    /// 1. If checkpoint exists, get next block body and return its first tx number.
    /// 2. If checkpoint doesn't exist, return 0.
    ///
    /// To get the range end: get last tx number for `to_block`.
    pub(crate) fn get_next_tx_num_range<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<DB>,
    ) -> ProviderResult<Option<RangeInclusive<TxNumber>>> {
        let from_tx_number = self.previous_checkpoint
            // Checkpoint exists, prune from the next transaction after the highest pruned one
            .and_then(|checkpoint| match checkpoint.tx_number {
                Some(tx_number) => Some(tx_number + 1),
                _ => {
                    error!(target: "pruner", ?checkpoint, "Expected transaction number in prune checkpoint, found None");
                    None
                },
            })
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let to_tx_number = match provider.block_body_indices(self.to_block)? {
            Some(body) => {
                let last_tx = body.last_tx_num();
                if last_tx + body.tx_count() == 0 {
                    // Prevents a scenario where the pruner correctly starts at a finalized block,
                    // but the first transaction (tx_num = 0) only appears on an non-finalized one.
                    // Should only happen on a test/hive scenario.
                    return Ok(None)
                }
                last_tx
            }
            None => return Ok(None),
        };

        let range = from_tx_number..=to_tx_number;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
    }

    /// Get next inclusive block range to prune according to the checkpoint, `to_block` block
    /// number and `limit`.
    ///
    /// To get the range start (`from_block`):
    /// 1. If checkpoint exists, use next block.
    /// 2. If checkpoint doesn't exist, use block 0.
    ///
    /// To get the range end: use block `to_block`.
    pub(crate) fn get_next_block_range(&self) -> Option<RangeInclusive<BlockNumber>> {
        let from_block = self.get_start_next_block_range();
        let range = from_block..=self.to_block;
        if range.is_empty() {
            return None
        }

        Some(range)
    }

    /// Returns the start of the next block range.
    ///
    /// 1. If checkpoint exists, use next block.
    /// 2. If checkpoint doesn't exist, use block 0.
    pub(crate) fn get_start_next_block_range(&self) -> u64 {
        self.previous_checkpoint
            .and_then(|checkpoint| checkpoint.block_number)
            // Checkpoint exists, prune from the next block after the highest pruned one
            .map(|block_number| block_number + 1)
            // No checkpoint exists, prune from genesis
            .unwrap_or(0)
    }
}

/// Segment pruning output, see [`Segment::prune`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PruneOutput {
    pub(crate) progress: PruneProgress,
    /// Number of entries pruned, i.e. deleted from the database.
    pub(crate) pruned: usize,
    /// Pruning checkpoint to save to database, if any.
    pub(crate) checkpoint: Option<PruneOutputCheckpoint>,
}

impl PruneOutput {
    /// Returns a [`PruneOutput`] with `done = true`, `pruned = 0` and `checkpoint = None`.
    /// Use when no pruning is needed.
    pub(crate) const fn done() -> Self {
        Self { progress: PruneProgress::Finished, pruned: 0, checkpoint: None }
    }

    /// Returns a [`PruneOutput`] with `done = false`, `pruned = 0` and `checkpoint = None`.
    /// Use when pruning is needed but cannot be done.
    pub(crate) const fn not_done(
        reason: PruneInterruptReason,
        checkpoint: Option<PruneOutputCheckpoint>,
    ) -> Self {
        Self { progress: PruneProgress::HasMoreData(reason), pruned: 0, checkpoint }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct PruneOutputCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub(crate) block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub(crate) tx_number: Option<TxNumber>,
}

impl PruneOutputCheckpoint {
    /// Converts [`PruneOutputCheckpoint`] to [`PruneCheckpoint`] with the provided [`PruneMode`]
    pub(crate) const fn as_prune_checkpoint(&self, prune_mode: PruneMode) -> PruneCheckpoint {
        PruneCheckpoint { block_number: self.block_number, tx_number: self.tx_number, prune_mode }
    }
}

impl From<PruneCheckpoint> for PruneOutputCheckpoint {
    fn from(checkpoint: PruneCheckpoint) -> Self {
        Self { block_number: checkpoint.block_number, tx_number: checkpoint.tx_number }
    }
}
