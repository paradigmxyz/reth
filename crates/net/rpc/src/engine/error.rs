use jsonrpsee::core::Error as RpcError;
use reth_primitives::{Bytes, H256, U256};
use thiserror::Error;

use crate::result::rpc_err;

/// Error returned by [`EngineApi`][crate::engine::EngineApi]
#[derive(Error, Debug)]
pub enum EngineApiError {
    #[error("Invalid payload extra data: {0}")]
    PayloadExtraData(Bytes),
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
    /// Unknown payload requested.
    #[error("Unknown payload")]
    PayloadUnknown,
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
        "Invalid transition terminal block hash. Execution: {execution:?}. Consensus: {consensus}"
    )]
    TerminalBlockHash {
        /// Execution terminal block hash. `None` if block number is not found in the database.
        execution: Option<H256>,
        /// Consensus terminal block hash.
        consensus: H256,
    },
    /// Encountered decoding error.
    #[error(transparent)]
    Decode(#[from] reth_rlp::DecodeError),
    /// API encountered an internal error.
    #[error(transparent)]
    Internal(#[from] reth_interfaces::Error),
}

impl EngineApiError {
    fn code(&self) -> i32 {
        match self {
            Self::PayloadUnknown => -38001,
            // Any other server error
            _ => jsonrpsee::types::error::INTERNAL_ERROR_CODE,
        }
    }
}

impl From<EngineApiError> for RpcError {
    fn from(value: EngineApiError) -> Self {
        rpc_err(value.code(), value.to_string(), None)
    }
}
