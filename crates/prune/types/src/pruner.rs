use alloy_primitives::{BlockNumber, TxNumber};

use crate::{PruneCheckpoint, PruneLimiter, PruneMode, PruneSegment};

/// Pruner run output.
#[derive(Debug)]
pub struct PrunerOutput {
    /// Pruning progress.
    pub progress: PruneProgress,
    /// Pruning output for each segment.
    pub segments: Vec<(PruneSegment, SegmentOutput)>,
}

impl From<PruneProgress> for PrunerOutput {
    fn from(progress: PruneProgress) -> Self {
        Self { progress, segments: Vec::new() }
    }
}

/// Segment pruning output.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SegmentOutput {
    /// Segment pruning progress.
    pub progress: PruneProgress,
    /// Number of entries pruned, i.e. deleted from the database.
    pub pruned: usize,
    /// Pruning checkpoint to save to database, if any.
    pub checkpoint: Option<SegmentOutputCheckpoint>,
}

impl SegmentOutput {
    /// Returns a [`SegmentOutput`] with `done = true`, `pruned = 0` and `checkpoint = None`.
    /// Use when no pruning is needed.
    pub const fn done() -> Self {
        Self { progress: PruneProgress::Finished, pruned: 0, checkpoint: None }
    }

    /// Returns a [`SegmentOutput`] with `done = false`, `pruned = 0` and `checkpoint = None`.
    /// Use when pruning is needed but cannot be done.
    pub const fn not_done(
        reason: PruneInterruptReason,
        checkpoint: Option<SegmentOutputCheckpoint>,
    ) -> Self {
        Self { progress: PruneProgress::HasMoreData(reason), pruned: 0, checkpoint }
    }
}

/// Segment pruning checkpoint.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct SegmentOutputCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub tx_number: Option<TxNumber>,
}

impl SegmentOutputCheckpoint {
    /// Converts [`PruneCheckpoint`] to [`SegmentOutputCheckpoint`].
    pub const fn from_prune_checkpoint(checkpoint: PruneCheckpoint) -> Self {
        Self { block_number: checkpoint.block_number, tx_number: checkpoint.tx_number }
    }

    /// Converts [`SegmentOutputCheckpoint`] to [`PruneCheckpoint`] with the provided [`PruneMode`]
    pub const fn as_prune_checkpoint(&self, prune_mode: PruneMode) -> PruneCheckpoint {
        PruneCheckpoint { block_number: self.block_number, tx_number: self.tx_number, prune_mode }
    }
}

/// Progress of pruning.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PruneProgress {
    /// There is more data to prune.
    HasMoreData(PruneInterruptReason),
    /// Pruning has been finished.
    Finished,
}

/// Reason for interrupting a prune run.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PruneInterruptReason {
    /// Prune run timed out.
    Timeout,
    /// Limit on the number of deleted entries (rows in the database) per prune run was reached.
    DeletedEntriesLimitReached,
    /// Unknown reason for stopping prune run.
    Unknown,
}

impl PruneInterruptReason {
    /// Creates new [`PruneInterruptReason`] based on the [`PruneLimiter`].
    pub fn new(limiter: &PruneLimiter) -> Self {
        if limiter.is_time_limit_reached() {
            Self::Timeout
        } else if limiter.is_deleted_entries_limit_reached() {
            Self::DeletedEntriesLimitReached
        } else {
            Self::Unknown
        }
    }

    /// Returns `true` if the reason is timeout.
    pub const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    /// Returns `true` if the reason is reaching the limit on deleted entries.
    pub const fn is_entries_limit_reached(&self) -> bool {
        matches!(self, Self::DeletedEntriesLimitReached)
    }
}

impl PruneProgress {
    /// Creates new [`PruneProgress`].
    ///
    /// If `done == true`, returns [`PruneProgress::Finished`], otherwise
    /// [`PruneProgress::HasMoreData`] is returned with [`PruneInterruptReason`] according to the
    /// passed limiter.
    pub fn new(done: bool, limiter: &PruneLimiter) -> Self {
        if done {
            Self::Finished
        } else {
            Self::HasMoreData(PruneInterruptReason::new(limiter))
        }
    }

    /// Returns `true` if prune run is finished.
    pub const fn is_finished(&self) -> bool {
        matches!(self, Self::Finished)
    }
}
