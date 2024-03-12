use jsonrpsee_types::error::{
    INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE, INVALID_PARAMS_MSG, SERVER_ERROR_MSG,
};
use reth_beacon_consensus::{BeaconForkChoiceUpdateError, BeaconOnNewPayloadError};
use reth_node_api::EngineObjectValidationError;
use reth_payload_builder::error::PayloadBuilderError;
use reth_primitives::{B256, U256};
use thiserror::Error;

/// The Engine API result type
pub type EngineApiResult<Ok> = Result<Ok, EngineApiError>;

/// Invalid payload attributes code.
pub const INVALID_PAYLOAD_ATTRIBUTES: i32 = -38003;
/// Payload unsupported fork code.
pub const UNSUPPORTED_FORK_CODE: i32 = -38005;
/// Payload unknown error code.
pub const UNKNOWN_PAYLOAD_CODE: i32 = -38001;
/// Request too large error code.
pub const REQUEST_TOO_LARGE_CODE: i32 = -38004;

/// Error message for the request too large error.
const REQUEST_TOO_LARGE_MESSAGE: &str = "Too large request";

/// Error message for the request too large error.
const INVALID_PAYLOAD_ATTRIBUTES_MSG: &str = "Invalid payload attributes";

/// Error returned by [`EngineApi`][crate::EngineApi]
///
/// Note: This is a high-fidelity error type which can be converted to an RPC error that adheres to
/// the [Engine API spec](https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#errors).
#[derive(Error, Debug)]
pub enum EngineApiError {
    // **IMPORTANT**: keep error messages in sync with the Engine API spec linked above.
    /// Payload does not exist / is not available.
    #[error("Unknown payload")]
    UnknownPayload,
    /// The payload body request length is too large.
    #[error("requested count too large: {len}")]
    PayloadRequestTooLarge {
        /// The length that was requested.
        len: u64,
    },
    /// Thrown if engine_getPayloadBodiesByRangeV1 contains an invalid range
    #[error("invalid start ({start}) or count ({count})")]
    InvalidBodiesRange {
        /// Start of the range
        start: u64,
        /// Requested number of items
        count: u64,
    },
    /// Terminal total difficulty mismatch during transition configuration exchange.
    #[error(
        "invalid transition terminal total difficulty: \
         execution: {execution}, consensus: {consensus}"
    )]
    TerminalTD {
        /// Execution terminal total difficulty value.
        execution: U256,
        /// Consensus terminal total difficulty value.
        consensus: U256,
    },
    /// Terminal block hash mismatch during transition configuration exchange.
    #[error(
        "invalid transition terminal block hash: \
         execution: {execution:?}, consensus: {consensus}"
    )]
    TerminalBlockHash {
        /// Execution terminal block hash. `None` if block number is not found in the database.
        execution: Option<B256>,
        /// Consensus terminal block hash.
        consensus: B256,
    },
    /// An error occurred while processing the fork choice update in the beacon consensus engine.
    #[error(transparent)]
    ForkChoiceUpdate(#[from] BeaconForkChoiceUpdateError),
    /// An error occurred while processing a new payload in the beacon consensus engine.
    #[error(transparent)]
    NewPayload(#[from] BeaconOnNewPayloadError),
    /// Encountered an internal error.
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// Fetching the payload failed
    #[error(transparent)]
    GetPayloadError(#[from] PayloadBuilderError),
    /// The payload or attributes are known to be malformed before processing.
    #[error(transparent)]
    EngineObjectValidationError(#[from] EngineObjectValidationError),
    /// If the optimism feature flag is enabled, the payload attributes must have a present
    /// gas limit for the forkchoice updated method.
    #[cfg(feature = "optimism")]
    #[error("Missing gas limit in payload attributes")]
    MissingGasLimitInPayloadAttributes,
}

/// Helper type to represent the `error` field in the error response:
/// <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#errors>
#[derive(serde::Serialize)]
struct ErrorData {
    err: String,
}

impl ErrorData {
    #[inline]
    fn new(err: impl std::fmt::Display) -> Self {
        Self { err: err.to_string() }
    }
}

impl From<EngineApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(error: EngineApiError) -> Self {
        match error {
            EngineApiError::InvalidBodiesRange { .. } |
            EngineApiError::EngineObjectValidationError(EngineObjectValidationError::Payload(
                _,
            )) |
            EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::InvalidParams(_),
            ) => {
                // Note: the data field is not required by the spec, but is also included by other
                // clients
                jsonrpsee_types::error::ErrorObject::owned(
                    INVALID_PARAMS_CODE,
                    INVALID_PARAMS_MSG,
                    Some(ErrorData::new(error)),
                )
            }
            EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::PayloadAttributes(_),
            ) => {
                // Note: the data field is not required by the spec, but is also included by other
                // clients
                jsonrpsee_types::error::ErrorObject::owned(
                    INVALID_PAYLOAD_ATTRIBUTES,
                    INVALID_PAYLOAD_ATTRIBUTES_MSG,
                    Some(ErrorData::new(error)),
                )
            }
            EngineApiError::UnknownPayload => jsonrpsee_types::error::ErrorObject::owned(
                UNKNOWN_PAYLOAD_CODE,
                error.to_string(),
                None::<()>,
            ),
            EngineApiError::PayloadRequestTooLarge { .. } => {
                jsonrpsee_types::error::ErrorObject::owned(
                    REQUEST_TOO_LARGE_CODE,
                    REQUEST_TOO_LARGE_MESSAGE,
                    Some(ErrorData::new(error)),
                )
            }
            EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::UnsupportedFork,
            ) => jsonrpsee_types::error::ErrorObject::owned(
                UNSUPPORTED_FORK_CODE,
                error.to_string(),
                None::<()>,
            ),
            // Error responses from the consensus engine
            EngineApiError::ForkChoiceUpdate(ref err) => match err {
                BeaconForkChoiceUpdateError::ForkchoiceUpdateError(err) => (*err).into(),
                BeaconForkChoiceUpdateError::EngineUnavailable |
                BeaconForkChoiceUpdateError::Internal(_) => {
                    jsonrpsee_types::error::ErrorObject::owned(
                        INTERNAL_ERROR_CODE,
                        SERVER_ERROR_MSG,
                        Some(ErrorData::new(error)),
                    )
                }
            },
            EngineApiError::NewPayload(ref err) => match err {
                BeaconOnNewPayloadError::Internal(_) => jsonrpsee_types::error::ErrorObject::owned(
                    INTERNAL_ERROR_CODE,
                    SERVER_ERROR_MSG,
                    Some(ErrorData::new(error)),
                ),
                BeaconOnNewPayloadError::PreCancunBlockWithBlobTransactions => {
                    jsonrpsee_types::error::ErrorObject::owned(
                        INVALID_PARAMS_CODE,
                        INVALID_PARAMS_MSG,
                        Some(ErrorData::new(error)),
                    )
                }
                BeaconOnNewPayloadError::EngineUnavailable => {
                    jsonrpsee_types::error::ErrorObject::owned(
                        INTERNAL_ERROR_CODE,
                        SERVER_ERROR_MSG,
                        Some(ErrorData::new(error)),
                    )
                }
            },
            // Optimism errors
            #[cfg(feature = "optimism")]
            EngineApiError::MissingGasLimitInPayloadAttributes => {
                jsonrpsee_types::error::ErrorObject::owned(
                    INVALID_PARAMS_CODE,
                    INVALID_PARAMS_MSG,
                    Some(ErrorData::new(error)),
                )
            }
            // Any other server error
            EngineApiError::TerminalTD { .. } |
            EngineApiError::TerminalBlockHash { .. } |
            EngineApiError::Internal(_) |
            EngineApiError::GetPayloadError(_) => jsonrpsee_types::error::ErrorObject::owned(
                INTERNAL_ERROR_CODE,
                SERVER_ERROR_MSG,
                Some(ErrorData::new(error)),
            ),
        }
    }
}

impl From<EngineApiError> for jsonrpsee_core::error::Error {
    fn from(error: EngineApiError) -> Self {
        jsonrpsee_core::error::Error::Call(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_rpc_types::engine::ForkchoiceUpdateError;

    #[track_caller]
    fn ensure_engine_rpc_error(
        code: i32,
        message: &str,
        err: impl Into<jsonrpsee_types::error::ErrorObject<'static>>,
    ) {
        let err = err.into();
        dbg!(&err);
        assert_eq!(err.code(), code);
        assert_eq!(err.message(), message);
    }

    // Tests that engine errors are formatted correctly according to the engine API spec
    // <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#errors>
    #[test]
    fn engine_error_rpc_error_test() {
        ensure_engine_rpc_error(
            UNSUPPORTED_FORK_CODE,
            "Unsupported fork",
            EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::UnsupportedFork,
            ),
        );

        ensure_engine_rpc_error(
            REQUEST_TOO_LARGE_CODE,
            "Too large request",
            EngineApiError::PayloadRequestTooLarge { len: 0 },
        );

        ensure_engine_rpc_error(
            -38002,
            "Invalid forkchoice state",
            EngineApiError::ForkChoiceUpdate(BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                ForkchoiceUpdateError::InvalidState,
            )),
        );

        ensure_engine_rpc_error(
            -38003,
            "Invalid payload attributes",
            EngineApiError::ForkChoiceUpdate(BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes,
            )),
        );

        ensure_engine_rpc_error(
            UNKNOWN_PAYLOAD_CODE,
            "Unknown payload",
            EngineApiError::UnknownPayload,
        );
    }
}
