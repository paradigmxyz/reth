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

/// Direction and target for pipeline operations.
#[derive(Debug, Clone, Copy)]
pub enum PipelineTarget {
    /// Target for forward synchronization.
    Sync(BlockHash),
    /// Target for backward unwinding.
    Unwind(BlockNumber),
}

impl PipelineTarget {
    /// Target for forward synchronization.
    pub fn sync_target(self) -> Option<BlockHash> {
        match self {
            PipelineTarget::Sync(hash) => Some(hash),
            PipelineTarget::Unwind(_) => None,
        }
    }
    /// Target for backward unwinding.
    pub fn unwind_target(self) -> Option<BlockNumber> {
        match self {
            PipelineTarget::Sync(_) => None,
            PipelineTarget::Unwind(number) => Some(number),
        }
    }
}

impl From<BlockHash> for PipelineTarget {
    fn from(hash: BlockHash) -> Self {
        Self::Sync(hash)
    }
}
