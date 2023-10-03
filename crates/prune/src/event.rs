use reth_primitives::{BlockNumber, PruneProgress, PruneSegment};
use std::{collections::BTreeMap, time::Duration};

/// An event emitted by a [Pruner][crate::Pruner].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PrunerEvent {
    /// Emitted when pruner finished running.
    Finished {
        tip_block_number: BlockNumber,
        elapsed: Duration,
        segments: BTreeMap<PruneSegment, (PruneProgress, usize)>,
    },
}
