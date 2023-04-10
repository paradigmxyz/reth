use jsonrpsee_types::error::{INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE};
use reth_beacon_consensus::BeaconEngineError;
use reth_primitives::{H256, U256};
use thiserror::Error;

/// The Engine API result type
pub type EngineApiResult<Ok> = Result<Ok, EngineApiError>;

/// Payload unknown error code.
pub const UNKNOWN_PAYLOAD_CODE: i32 = -38001;
/// Request too large error code.
pub const REQUEST_TOO_LARGE_CODE: i32 = -38004;

/// Error returned by [`EngineApi`][crate::EngineApi]
#[derive(Error, Debug)]
pub enum EngineApiError {
    /// Unknown payload requested.
    #[error("Unknown payload")]
    PayloadUnknown,
    /// The payload body request length is too large.
    #[error("Payload request too large: {len}")]
    PayloadRequestTooLarge {
        /// The length that was requested.
        len: u64,
    },
    /// Thrown if engine_getPayloadBodiesByRangeV1 contains an invalid range
    #[error("invalid start or count, start: {start} count: {count}")]
    InvalidBodiesRange {
        /// Start of the range
        start: u64,
        /// requested number of items
        count: u64,
    },
    /// Thrown if engine_forkchoiceUpdatedV1 contains withdrawals
    #[error("withdrawals not supported in V1")]
    WithdrawalsNotSupportedInV1,
    /// Thrown if engine_forkchoiceUpdated contains no withdrawals after Shanghai
    #[error("no withdrawals post-shanghai")]
    NoWithdrawalsPostShanghai,
    /// Thrown if engine_forkchoiceUpdated contains withdrawals before Shanghai
    #[error("withdrawals pre-shanghai")]
    HasWithdrawalsPreShanghai,
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
    /// Beacon consensus engine error.
    #[error(transparent)]
    ConsensusEngine(#[from] BeaconEngineError),
    /// Encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl From<EngineApiError> for jsonrpsee_types::error::CallError {
    fn from(error: EngineApiError) -> Self {
        let code = match error {
            EngineApiError::InvalidBodiesRange { .. } |
            EngineApiError::WithdrawalsNotSupportedInV1 |
            EngineApiError::NoWithdrawalsPostShanghai |
            EngineApiError::HasWithdrawalsPreShanghai => INVALID_PARAMS_CODE,
            EngineApiError::PayloadUnknown => UNKNOWN_PAYLOAD_CODE,
            EngineApiError::PayloadRequestTooLarge { .. } => REQUEST_TOO_LARGE_CODE,
            // Any other server error
            _ => INTERNAL_ERROR_CODE,
        };
        jsonrpsee_types::error::CallError::Custom(jsonrpsee_types::error::ErrorObject::owned(
            code,
            error.to_string(),
            None::<()>,
        ))
    }
}

impl From<EngineApiError> for jsonrpsee_core::error::Error {
    fn from(error: EngineApiError) -> Self {
        jsonrpsee_types::error::CallError::from(error).into()
    }
}
