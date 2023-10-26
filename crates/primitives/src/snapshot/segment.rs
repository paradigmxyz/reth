use crate::{BlockNumber, TxNumber};
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Segment of the data that can be snapshotted.
pub enum SnapshotSegment {
    /// Snapshot segment responsible for the `CanonicalHeaders`, `Headers`, `HeaderTD` tables.
    Headers,
    /// Snapshot segment responsible for the `Transactions` table.
    Transactions,
    /// Snapshot segment responsible for the `Receipts` table.
    Receipts,
}

/// A segment header that contains information common to all segments. Used for storage.
#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentHeader {
    block_range: RangeInclusive<BlockNumber>,
    tx_range: RangeInclusive<TxNumber>,
}

impl SegmentHeader {
    /// Returns [`SegmentHeader`].
    pub fn new(
        block_range: RangeInclusive<BlockNumber>,
        tx_range: RangeInclusive<TxNumber>,
    ) -> Self {
        Self { block_range, tx_range }
    }

    /// Returns the first block number of the segment.
    pub fn block_start(&self) -> BlockNumber {
        *self.block_range.start()
    }

    /// Returns the first transaction number of the segment.
    pub fn tx_start(&self) -> TxNumber {
        *self.tx_range.start()
    }
}
