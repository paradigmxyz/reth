//! State transition primitives.

use alloy_primitives::BlockNumber;

/// Type of the state transition.
#[derive(
    serde::Serialize, serde::Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug,
)]
pub enum TransitionType {
    /// Pre block transition.
    PreBlock,
    /// Contains transaction index denoting its position within the block.
    Transaction(u32),
    /// Final block transition.
    PostBlock,
}

/// Unique identifier of the state transition.
/// It is composed of block number and the transition type.
#[derive(
    serde::Serialize, serde::Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug,
)]
pub struct TransitionId(
    /// Number of the block transition belongs to.
    pub BlockNumber,
    /// Type of the transition.
    pub TransitionType,
);

impl TransitionId {
    /// Create a pre-block state transition id.
    pub fn pre_block(block_number: BlockNumber) -> Self {
        Self(block_number, TransitionType::PreBlock)
    }

    /// Create a transaction state transition id.
    pub fn transaction(block_number: BlockNumber, idx: u32) -> Self {
        Self(block_number, TransitionType::Transaction(idx))
    }

    /// Create a post-block state transition id.
    pub fn post_block(block_number: BlockNumber) -> Self {
        Self(block_number, TransitionType::PostBlock)
    }
}
