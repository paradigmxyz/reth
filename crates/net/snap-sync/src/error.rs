//! Snap sync error types.

use alloy_primitives::B256;
use reth_db_api::DatabaseError;
use reth_network_p2p::error::RequestError;
use reth_storage_errors::provider::ProviderError;

/// Errors that can occur during snap sync.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// The computed state root does not match the pivot block's state root.
    #[error("state root mismatch: expected {expected}, got {got}")]
    StateRootMismatch {
        /// Expected state root from pivot header.
        expected: B256,
        /// Computed state root after snap sync.
        got: B256,
    },
    /// A peer returned an invalid or inconsistent account range.
    #[error("invalid account range response: {0}")]
    InvalidAccountRange(String),
    /// A peer returned an invalid or inconsistent storage range.
    #[error("invalid storage range response: {0}")]
    InvalidStorageRange(String),
    /// A peer returned invalid bytecodes.
    #[error("invalid bytecode response: {0}")]
    InvalidBytecode(String),
    /// No peers available that support the snap protocol.
    #[error("no snap-capable peers available")]
    NoPeers,
    /// Network request failed.
    #[error("network request failed: {0}")]
    Request(#[from] RequestError),
    /// Database/provider error.
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    /// Pivot block not found.
    #[error("pivot block {0} not found")]
    PivotNotFound(B256),
    /// RLP decoding error.
    #[error("rlp decode error: {0}")]
    RlpDecode(#[from] alloy_rlp::Error),
    /// Database error.
    #[error("database error: {0}")]
    Database(#[from] DatabaseError),
    /// State root verification failed.
    #[error("state root verification error: {0}")]
    StateRootVerification(String),
    /// Snap sync was cancelled.
    #[error("snap sync cancelled")]
    Cancelled,
}
