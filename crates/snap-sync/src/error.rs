//! Reports failures that make a partial Snap generation unsafe to resume.
//!
//! Layout, phase, and marker errors remain distinct from provider failures for retry decisions.

use crate::SnapPhase;
use alloy_primitives::B256;

/// Error returned while assembling a Snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// No eligible peer can currently serve the requested Snap data.
    #[error(transparent)]
    Request(#[from] reth_network_p2p::error::RequestError),
    /// Locally assembled request bounds or inputs are inconsistent.
    #[error("invalid snap request: {0}")]
    InvalidRequest(String),
    /// A BAL was delivered out of durable block order.
    #[error("snap BAL block {actual} does not advance expected block {expected}")]
    UnexpectedBlock {
        /// Persisted next block.
        expected: u64,
        /// Delivered block.
        actual: u64,
    },
    /// The generation anchor or a requested BAL header left the canonical chain.
    #[error("canonical header {block_number} is {actual:?}, expected {expected}")]
    CanonicalHeaderMismatch {
        /// Header number being checked.
        block_number: u64,
        /// Header hash authenticated by the generation or response.
        expected: B256,
        /// Current canonical hash, if the header still exists.
        actual: Option<B256>,
    },
    /// A required canonical header has not been downloaded yet.
    #[error("canonical snap header {0} is unavailable")]
    MissingHeader(u64),
    /// The generation root no longer matches its canonical target header.
    #[error("canonical state root at block {block_number} is {actual}, expected {expected}")]
    CanonicalStateRootMismatch {
        /// Target header number.
        block_number: u64,
        /// Root retained by the durable generation.
        expected: B256,
        /// Root currently committed by the header.
        actual: B256,
    },
    /// Trie generation has not durably reached its target checkpoint.
    #[error("snap trie checkpoint is {actual:?}, expected {expected}")]
    TrieIncomplete {
        /// Target generation block.
        expected: u64,
        /// Current Merkle stage block, if present.
        actual: Option<u64>,
    },
    /// Reth's Merkle stage rejected the downloaded state.
    #[error("snap trie generation failed: {0}")]
    Trie(String),
    /// The database uses plain canonical state, which hashed Snap keys cannot populate.
    #[error("snap sync requires the v2 hashed-state layout")]
    UnsupportedStorageLayout,
    /// A generation operation was attempted in the wrong phase.
    #[error("snap generation is in {actual:?}, expected {expected:?}")]
    UnexpectedPhase {
        /// Required phase.
        expected: SnapPhase,
        /// Persisted phase.
        actual: SnapPhase,
    },
    /// A range completed after another operation advanced the generation.
    #[error("snap generation changed while a range was in flight")]
    StaleGeneration,
    /// A continuation must move past the committed range.
    #[error("snap account cursor {next} does not advance {current}")]
    NonAdvancingAccountCursor {
        /// Persisted inclusive origin.
        current: B256,
        /// Proposed inclusive origin.
        next: B256,
    },
    /// The persisted generation marker cannot be resumed safely.
    #[error("invalid snap generation marker: {0}")]
    InvalidGeneration(String),
    /// The provider rejected a database operation.
    #[error("snap database operation failed: {0}")]
    Database(String),
}

// Erasing provider error types keeps the coordinator's public bounds small.
pub(crate) fn db_error(error: impl core::fmt::Display) -> SnapSyncError {
    SnapSyncError::Database(error.to_string())
}
