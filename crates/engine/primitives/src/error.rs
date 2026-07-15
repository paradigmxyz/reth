use alloc::boxed::Box;
use alloy_primitives::BlockNumber;
use alloy_rpc_types_engine::ForkchoiceUpdateError;

/// Represents all error cases when handling a new payload.
///
/// This represents all possible error cases that must be returned as JSON RPC errors back to the
/// beacon node.
#[derive(Debug, thiserror::Error)]
pub enum BeaconOnNewPayloadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
    /// An internal error occurred, not necessarily related to the payload.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl BeaconOnNewPayloadError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}

/// Represents error cases for an applied forkchoice update.
///
/// This represents all possible error cases, that must be returned as JSON RPC errors back to the
/// beacon node.
#[derive(Debug, thiserror::Error)]
pub enum BeaconForkChoiceUpdateError {
    /// Thrown when a forkchoice update resulted in an error.
    #[error("forkchoice update error: {0}")]
    ForkchoiceUpdateError(#[from] ForkchoiceUpdateError),
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
    /// An internal error occurred, not necessarily related to the update.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl BeaconForkChoiceUpdateError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}

/// Represents error cases when setting the canonical head through the debug API.
#[derive(Debug, thiserror::Error)]
pub enum DebugSetHeadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("consensus engine task stopped")]
    EngineUnavailable,
    /// The requested canonical block does not exist.
    #[error("block {0} not found")]
    BlockNotFound(BlockNumber),
    /// The requested block is below the finalized block.
    #[error("cannot set head to block {target} below finalized block {finalized}")]
    Finalized {
        /// The requested block number.
        target: BlockNumber,
        /// The current finalized block number.
        finalized: BlockNumber,
    },
    /// The pipeline is actively syncing and owns canonical chain progress.
    #[error("cannot set head while pipeline sync is active")]
    Syncing,
    /// An internal error occurred while updating the canonical head.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl DebugSetHeadError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}
