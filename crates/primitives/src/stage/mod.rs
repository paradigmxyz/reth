//! Staged sync primitives.

mod id;
use crate::{BlockHash, BlockNumber};
pub use id::StageId;

mod checkpoints;
pub use checkpoints::{
    AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint,
    HeadersCheckpoint, IndexHistoryCheckpoint, MerkleCheckpoint, StageCheckpoint,
    StageUnitCheckpoint, StorageHashingCheckpoint,
};

/// Direction and target block for pipeline operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineTarget {
    /// Target for forward synchronization, indicating a block hash to sync to.
    Sync(BlockHash),
    /// Target for backward unwinding, indicating a block number to unwind to.
    Unwind(BlockNumber),
}

impl PipelineTarget {
    /// Returns the target block hash for forward synchronization, if applicable.
    ///
    /// # Returns
    ///
    /// - `Some(BlockHash)`: The target block hash for forward synchronization.
    /// - `None`: If the target is for backward unwinding.
    pub const fn sync_target(self) -> Option<BlockHash> {
        match self {
            Self::Sync(hash) => Some(hash),
            Self::Unwind(_) => None,
        }
    }

    /// Returns the target block number for backward unwinding, if applicable.
    ///
    /// # Returns
    ///
    /// - `Some(BlockNumber)`: The target block number for backward unwinding.
    /// - `None`: If the target is for forward synchronization.
    pub const fn unwind_target(self) -> Option<BlockNumber> {
        match self {
            Self::Sync(_) => None,
            Self::Unwind(number) => Some(number),
        }
    }
}

impl From<BlockHash> for PipelineTarget {
    fn from(hash: BlockHash) -> Self {
        Self::Sync(hash)
    }
}

impl std::fmt::Display for PipelineTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sync(block) => {
                write!(f, "Sync({block})")
            }
            Self::Unwind(block) => write!(f, "Unwind({block})"),
        }
    }
}
