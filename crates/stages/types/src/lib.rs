//! Commonly used types for staged sync usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod id;
use alloy_primitives::{BlockHash, BlockNumber};
pub use id::StageId;

mod checkpoints;
pub use checkpoints::{
    AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint,
    HeadersCheckpoint, IndexHistoryCheckpoint, MerkleCheckpoint, StageCheckpoint,
    StageUnitCheckpoint, StorageHashingCheckpoint,
};

mod execution;
pub use execution::*;

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
