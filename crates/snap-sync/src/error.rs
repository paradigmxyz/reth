//! Failures raised while assembling a snap state generation.

use alloy_primitives::B256;
use reth_network_p2p::error::RequestError;
use reth_storage_api::SnapAttemptId;
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};

/// Error returned while assembling a snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// A header lookup failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// A request for state failed.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The storage layout keys state by address, which snap cannot fill in without preimages.
    #[error("snap synchronization requires the hashed state layout")]
    UnsupportedStorage,
    /// No attempt owns the persisted state.
    #[error("no snap attempt owns the persisted state")]
    NoAttempt,
    /// A write was presented for an attempt or pivot that no longer owns the persisted state.
    #[error("stale snap write for attempt {attempt} at state version {state_version}")]
    StaleWrite {
        /// Attempt the rejected write claims.
        attempt: SnapAttemptId,
        /// State version the rejected write was proved against.
        state_version: u64,
    },
    /// A range was proved against a root other than the one the attempt downloads.
    #[error("range proved against {got} while the attempt downloads {expected}")]
    RootMismatch {
        /// Root the attempt currently downloads.
        expected: B256,
        /// Root the range was proved against.
        got: B256,
    },
    /// No account coverage is recorded for the attempt.
    #[error("no account coverage is recorded for the attempt")]
    NoCoverage,
    /// A persisted coverage record this build cannot read.
    #[error("account coverage record version {version:?} is not supported")]
    UnsupportedCoverage {
        /// Version found on disk, absent when the record carries no numeric version.
        version: Option<u64>,
    },
    /// A range was downloaded from somewhere other than the next key to cover.
    #[error("account range starts at {got}, coverage continues at {expected:?}")]
    OutOfOrderRange {
        /// Key the coverage continues at, or none once it is complete.
        expected: Option<B256>,
        /// Key the range was requested from.
        got: B256,
    },
    /// A verified range proved nothing past the key it was requested from.
    #[error("account range made no progress past {origin}")]
    NoProgress {
        /// Key the range was requested from.
        origin: B256,
    },
    /// An account with storage was committed without it.
    #[error("account {account} has storage that was not downloaded")]
    MissingStorage {
        /// Hashed address of the account.
        account: B256,
    },
    /// Storage was supplied for an account that is not in the range or has none.
    #[error("storage supplied for {account}, which is not a contract in the range")]
    UnexpectedStorage {
        /// Hashed address the storage was supplied for.
        account: B256,
    },
    /// An account's code is neither supplied nor stored.
    #[error("code {hash} is neither supplied nor stored")]
    MissingCode {
        /// Hash of the missing code.
        hash: B256,
    },
}

impl From<DatabaseError> for SnapSyncError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(error.into())
    }
}
