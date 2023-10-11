use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SnapshotSegment {
    Headers,
    Transactions,
    Receipts,
}
