use crate::{PruneCheckpoint, PruneMode, PruneSegment};
use alloc::{format, string::ToString, vec::Vec};
use alloy_primitives::{BlockNumber, TxNumber};
use core::time::Duration;
use derive_more::Display;
use tracing::debug;

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

impl PrunerOutput {
    /// Logs a human-readable summary of the pruner run at DEBUG level.
    ///
    /// Format: `"Pruner finished tip=24328929 deleted=10886 elapsed=148ms
    /// segments=AccountHistory[24318865, done] ..."`
    #[inline]
    pub fn debug_log(
        &self,
        tip_block_number: BlockNumber,
        deleted_entries: usize,
        elapsed: Duration,
    ) {
        let message = match self.progress {
            PruneProgress::HasMoreData(_) => "Pruner interrupted, has more data",
            PruneProgress::Finished => "Pruner finished",
        };

        let segments: Vec<_> = self
            .segments
            .iter()
            .filter(|(_, seg)| seg.pruned > 0)
            .map(|(segment, seg)| {
                let block = seg
                    .checkpoint
                    .and_then(|c| c.block_number)
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let status = if seg.progress.is_finished() { "done" } else { "in_progress" };
                format!("{segment}[{block}, {status}]")
            })
            .collect();

        debug!(
            target: "pruner",
            %tip_block_number,
            deleted_entries,
            ?elapsed,
            segments = %segments.join(" "),
            "{message}",
        );
    }
}

/// Represents information of a pruner run for a segment.
#[derive(Debug, Clone, PartialEq, Eq, Display)]
#[display("(table={segment}, pruned={pruned}, status={progress})")]
pub struct PrunedSegmentInfo {
    /// The pruned segment
    pub segment: PruneSegment,
    /// Number of pruned entries
    pub pruned: usize,
    /// Prune progress
    pub progress: PruneProgress,
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

    /// Returns a [`SegmentOutput`] with `done = false`, `pruned = 0` and the given checkpoint.
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
#[derive(Debug, PartialEq, Eq, Clone, Copy, Display)]
pub enum PruneProgress {
    /// There is more data to prune.
    #[display("HasMoreData({_0})")]
    HasMoreData(PruneInterruptReason),
    /// Pruning has been finished.
    #[display("Finished")]
    Finished,
}

/// Reason for interrupting a prune run.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Display)]
pub enum PruneInterruptReason {
    /// Prune run timed out.
    Timeout,
    /// Limit on the number of deleted entries (rows in the database) per prune run was reached.
    DeletedEntriesLimitReached,
    /// Waiting for another segment to finish pruning before this segment can proceed.
    WaitingOnSegment(PruneSegment),
    /// Unknown reason for stopping prune run.
    Unknown,
}

impl PruneInterruptReason {
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
    /// Returns `true` if prune run is finished.
    pub const fn is_finished(&self) -> bool {
        matches!(self, Self::Finished)
    }

    /// Combines two progress values, keeping `HasMoreData` if either has it.
    ///
    /// Once any segment reports `HasMoreData`, the combined progress remains
    /// `HasMoreData`. Only returns `Finished` if both are `Finished`.
    #[must_use]
    pub const fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::HasMoreData(reason), _) => Self::HasMoreData(reason),
            (Self::Finished, other) => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prune_progress_combine() {
        use PruneInterruptReason::*;
        use PruneProgress::*;

        // HasMoreData dominates Finished
        assert!(matches!(HasMoreData(Timeout).combine(Finished), HasMoreData(Timeout)));

        // First HasMoreData reason is preserved
        assert!(matches!(
            HasMoreData(Timeout).combine(HasMoreData(DeletedEntriesLimitReached)),
            HasMoreData(Timeout)
        ));

        // Finished adopts new progress
        assert!(matches!(Finished.combine(Finished), Finished));
        assert!(matches!(
            Finished.combine(HasMoreData(DeletedEntriesLimitReached)),
            HasMoreData(DeletedEntriesLimitReached)
        ));
    }
}
