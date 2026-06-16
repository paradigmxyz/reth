use alloc::boxed::Box;
use alloy_rpc_types_engine::ForkchoiceUpdateError;
use reth_errors::ConsensusError;
use reth_storage_api::errors::ProviderError;

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

/// Error returned by
/// [`PayloadValidator::validate_block_post_execution_with_hashed_state`](crate::PayloadValidator::validate_block_post_execution_with_hashed_state).
///
/// Distinguishes a block that violates a post-execution consensus rule from an internal failure to
/// load the state needed to run the check, so the engine does not mark a block invalid over a
/// transient provider error.
#[derive(Debug, thiserror::Error)]
pub enum PostExecutionValidationError {
    /// The block violates a post-execution consensus rule.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Loading the state required to validate the block failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}
