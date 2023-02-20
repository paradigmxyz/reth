use reth_primitives::{Bytes, H256, U256};
use thiserror::Error;

/// The Engine API result type
pub type EngineApiResult<Ok> = Result<Ok, EngineApiError>;

/// Payload unknown error code.
pub const UNKNOWN_PAYLOAD_CODE: i32 = -38001;
/// Request too large error code.
pub const REQUEST_TOO_LARGE_CODE: i32 = -38004;

/// Error returned by [`EngineApi`][crate::EngineApi]
#[derive(Error, PartialEq, Debug)]
pub enum EngineApiError {
    /// Invalid payload extra data.
    #[error("Invalid payload extra data: {0}")]
    PayloadExtraData(Bytes),
    /// Invalid payload base fee.
    #[error("Invalid payload base fee: {0}")]
    PayloadBaseFee(U256),
    /// Invalid payload block hash.
    #[error("Invalid payload block hash. Execution: {execution}. Consensus: {consensus}")]
    PayloadBlockHash {
        /// The block hash computed from the payload.
        execution: H256,
        /// The block hash provided with the payload.
        consensus: H256,
    },
    /// Invalid payload block hash.
    #[error("Invalid payload timestamp: {invalid}. Latest: {latest}")]
    PayloadTimestamp {
        /// The payload timestamp.
        invalid: u64,
        /// Latest available timestamp.
        latest: u64,
    },
    /// Failed to recover transaction signer.
    #[error("Failed to recover signer for payload transaction: {hash:?}")]
    PayloadSignerRecovery {
        /// The hash of the failed transaction
        hash: H256,
    },
    /// Received pre-merge payload.
    #[error("Received pre-merge payload.")]
    PayloadPreMerge,
    /// Unknown payload requested.
    #[error("Unknown payload")]
    PayloadUnknown,
    /// The payload body request length is too large.
    #[error("Payload request too large: {len}")]
    PayloadRequestTooLarge {
        /// The length that was requested.
        len: u64,
    },
    /// The params are invalid.
    #[error("Invalid params")]
    InvalidParams,
    /// Terminal total difficulty mismatch during transition configuration exchange.
    #[error(
        "Invalid transition terminal total difficulty. Execution: {execution}. Consensus: {consensus}"
    )]
    TerminalTD {
        /// Execution terminal total difficulty value.
        execution: U256,
        /// Consensus terminal total difficulty value.
        consensus: U256,
    },
    /// Terminal block hash mismatch during transition configuration exchange.
    #[error(
        "Invalid transition terminal block hash. Execution: {execution:?}. Consensus: {consensus:?}"
    )]
    TerminalBlockHash {
        /// Execution terminal block hash. `None` if block number is not found in the database.
        execution: Option<H256>,
        /// Consensus terminal block hash.
        consensus: H256,
    },
    /// Forkchoice zero hash head received.
    #[error("Received zero hash as forkchoice head")]
    ForkchoiceEmptyHead,
    /// Chain spec merge terminal total difficulty is not set
    #[error("The merge terminal total difficulty is not known")]
    UnknownMergeTerminalTotalDifficulty,
    /// Encountered decoding error.
    #[error(transparent)]
    Decode(#[from] reth_rlp::DecodeError),
    /// API encountered an internal error.
    #[error(transparent)]
    Internal(#[from] reth_interfaces::Error),
}
