use crate::PrunedSegmentInfo;
use alloc::vec::Vec;
use alloy_primitives::BlockNumber;
use core::time::Duration;

/// An event emitted by a pruner.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PrunerEvent {
    /// Emitted when pruner started running.
    Started {
        /// The tip block number before pruning.
        tip_block_number: BlockNumber,
    },
    /// Emitted when pruner finished running.
    Finished {
        /// The tip block number before pruning.
        tip_block_number: BlockNumber,
        /// The elapsed time for the pruning process.
        elapsed: Duration,
        /// Collected pruning stats.
        stats: Vec<PrunedSegmentInfo>,
    },
}
