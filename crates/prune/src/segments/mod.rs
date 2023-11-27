mod account_history;
mod headers;
mod history;
mod receipts;
mod receipts_by_logs;
mod sender_recovery;
mod set;
mod storage_history;
mod transaction_lookup;
mod transactions;

pub use account_history::AccountHistory;
pub use headers::Headers;
pub use receipts::Receipts;
pub use receipts_by_logs::ReceiptsByLogs;
pub use sender_recovery::SenderRecovery;
pub use set::SegmentSet;
use std::fmt::Debug;
pub use storage_history::StorageHistory;
pub use transaction_lookup::TransactionLookup;
pub use transactions::Transactions;

use crate::PrunerError;
use reth_db::database::Database;
use reth_interfaces::{provider::ProviderResult, RethResult};
use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, TxNumber};
use reth_provider::{BlockReader, DatabaseProviderRW, PruneCheckpointWriter};
use std::ops::RangeInclusive;
use tracing::error;

/// A segment represents a pruning of some portion of the data.
///
/// Segments are called from [Pruner](crate::Pruner) with the following lifecycle:
/// 1. Call [Segment::prune] with `delete_limit` of [PruneInput].
/// 2. If [Segment::prune] returned a [Some] in `checkpoint` of [PruneOutput], call
///    [Segment::save_checkpoint].
/// 3. Subtract `pruned` of [PruneOutput] from `delete_limit` of next [PruneInput].
pub trait Segment<DB: Database>: Debug + Send + Sync {
    /// Segment of data that's pruned.
    fn segment(&self) -> PruneSegment;

    /// Prune mode with which the segment was initialized
    fn mode(&self) -> Option<PruneMode>;

    /// Prune data for [Self::segment] using the provided input.
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError>;

    /// Save checkpoint for [Self::segment] to the database.
    fn save_checkpoint(
        &self,
        provider: &DatabaseProviderRW<DB>,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        provider.save_prune_checkpoint(self.segment(), checkpoint)
    }
}

/// Segment pruning input, see [Segment::prune].
#[derive(Debug, Clone, Copy)]
pub struct PruneInput {
    pub(crate) previous_checkpoint: Option<PruneCheckpoint>,
    /// Target block up to which the pruning needs to be done, inclusive.
    pub(crate) to_block: BlockNumber,
    /// Maximum entries to delete from the database.
    pub(crate) delete_limit: usize,
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
    ) -> RethResult<Option<RangeInclusive<TxNumber>>> {
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
            Some(body) => body,
            None => return Ok(None),
        }
        .last_tx_num();

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
        let from_block = self
            .previous_checkpoint
            .and_then(|checkpoint| checkpoint.block_number)
            // Checkpoint exists, prune from the next block after the highest pruned one
            .map(|block_number| block_number + 1)
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let range = from_block..=self.to_block;
        if range.is_empty() {
            return None
        }

        Some(range)
    }
}

/// Segment pruning output, see [Segment::prune].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PruneOutput {
    /// `true` if pruning has been completed up to the target block, and `false` if there's more
    /// data to prune in further runs.
    pub(crate) done: bool,
    /// Number of entries pruned, i.e. deleted from the database.
    pub(crate) pruned: usize,
    /// Pruning checkpoint to save to database, if any.
    pub(crate) checkpoint: Option<PruneOutputCheckpoint>,
}

impl PruneOutput {
    /// Returns a [PruneOutput] with `done = true`, `pruned = 0` and `checkpoint = None`.
    /// Use when no pruning is needed.
    pub(crate) const fn done() -> Self {
        Self { done: true, pruned: 0, checkpoint: None }
    }

    /// Returns a [PruneOutput] with `done = false`, `pruned = 0` and `checkpoint = None`.
    /// Use when pruning is needed but cannot be done.
    pub(crate) const fn not_done() -> Self {
        Self { done: false, pruned: 0, checkpoint: None }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct PruneOutputCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub(crate) block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub(crate) tx_number: Option<TxNumber>,
}

impl PruneOutputCheckpoint {
    /// Converts [PruneOutputCheckpoint] to [PruneCheckpoint] with the provided [PruneMode]
    pub(crate) fn as_prune_checkpoint(&self, prune_mode: PruneMode) -> PruneCheckpoint {
        PruneCheckpoint { block_number: self.block_number, tx_number: self.tx_number, prune_mode }
    }
}
