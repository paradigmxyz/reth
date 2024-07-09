use alloy_primitives::BlockNumber;
use reth_prune_types::{PruneProgress, PruneSegment};
use std::time::Duration;

/// An event emitted by a [Pruner][crate::Pruner].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PrunerEvent {
    /// Emitted when pruner started running.
    Started { tip_block_number: BlockNumber },
    /// Emitted when pruner finished running.
    Finished {
        tip_block_number: BlockNumber,
        elapsed: Duration,
        stats: Vec<(PruneSegment, usize, PruneProgress)>,
    },
}
