//! Failures raised while assembling a snap state generation.

use alloy_primitives::B256;
use reth_downloaders::snap::{InvalidBlockAccessListRequest, InvalidStorageRangeRequest};
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
    /// A storage request did not match the accounts it was built from.
    #[error(transparent)]
    StorageRequest(#[from] InvalidStorageRangeRequest),
    /// A block access list request did not match the headers it was built from.
    #[error(transparent)]
    BlockAccessListRequest(#[from] InvalidBlockAccessListRequest),
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
    /// No catch-up progress is recorded for the attempt.
    #[error("no catch-up progress is recorded for the attempt")]
    NoCatchUpProgress,
    /// A block the catch-up needs has no header.
    #[error("block {block} has no header to authenticate its access list against")]
    MissingHeader {
        /// Block the header is missing for.
        block: u64,
    },
    /// A list was applied for a block other than the one continuing the applied sequence.
    #[error("block access list for block {got}, the applied state continues at {expected}")]
    OutOfOrderBlock {
        /// Block the applied state continues at.
        expected: u64,
        /// Block the list was applied for.
        got: u64,
    },
    /// A list was applied for a block past the pivot.
    #[error("block access list for block {block} past pivot {pivot}")]
    BlockPastPivot {
        /// Block the attempt is anchored to.
        pivot: u64,
        /// Block the list was applied for.
        block: u64,
    },
    /// The pivot was moved to a block not past the current one.
    #[error("pivot {pivot} cannot move to block {target}, which is not past it")]
    PivotNotAdvanced {
        /// Block the attempt is anchored to.
        pivot: u64,
        /// Block the pivot was moved to.
        target: u64,
    },
    /// Storage persisted ahead of a range was committed before catch-up reached the pivot.
    #[error("catch-up applied block {applied}, the pivot is {pivot}")]
    CatchUpBehindPivot {
        /// Last block whose list is applied.
        applied: u64,
        /// Block the attempt is anchored to.
        pivot: u64,
    },
    /// A list was applied for a block the canonical chain no longer holds.
    #[error("block {block} ({hash}) is no longer canonical")]
    NonCanonicalBlock {
        /// Number of the block.
        block: u64,
        /// Hash of the block the list belongs to.
        hash: B256,
    },
    /// A list was applied for a block building on another chain than the applied state.
    #[error("block access list for a block building on {got}, the applied state is at {expected}")]
    ForkedBlock {
        /// Hash of the last applied block.
        expected: B256,
        /// Hash the block the list belongs to builds on.
        got: B256,
    },
    /// No account coverage is recorded for the attempt.
    #[error("no account coverage is recorded for the attempt")]
    NoCoverage,
    /// A persisted progress record this build cannot read.
    #[error("{key} record version {version:?} is not supported")]
    UnsupportedRecord {
        /// Metadata key the record is stored under.
        key: &'static str,
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
    #[error("range made no progress past {origin}")]
    NoProgress {
        /// Key the range was requested from.
        origin: B256,
    },
    /// Storage was downloaded from somewhere other than where its contract's progress continues.
    #[error("storage for {account} from {from} does not continue its persisted progress")]
    OutOfOrderStorage {
        /// Hashed address of the contract.
        account: B256,
        /// Slot the storage was requested from.
        from: B256,
    },
    /// An account with storage was committed without it.
    #[error("account {account} has storage that was not downloaded")]
    MissingStorage {
        /// Hashed address of the account.
        account: B256,
    },
    /// The storage supplied for an account does not hash to the root the account commits to.
    #[error("storage supplied for {account} hashes to {got}, its account commits to {expected}")]
    StorageRootMismatch {
        /// Hashed address of the account.
        account: B256,
        /// Storage root the account commits to.
        expected: B256,
        /// Root of the supplied storage.
        got: B256,
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
    /// Code was supplied under a hash it does not hash to.
    #[error("code supplied as {expected} hashes to {got}")]
    CodeMismatch {
        /// Hash the code was supplied under.
        expected: B256,
        /// Hash of the supplied code.
        got: B256,
    },
}

impl From<DatabaseError> for SnapSyncError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(error.into())
    }
}
