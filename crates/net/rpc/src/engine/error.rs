use jsonrpsee::core::Error as RpcError;
use reth_primitives::{H256, U256};
use thiserror::Error;

use crate::result::rpc_err;

/// Error returned by [`EngineApi`][crate::engine::EngineApi]
#[derive(Error, Debug)]
pub enum EngineApiError {
    #[error(
        "Invalid transition terminal total difficulty. Execution: {execution}. Consensus: {consensus}"
    )]
    TerminalTD { execution: U256, consensus: U256 },
    #[error(
        "Invalid transition terminal block hash. Execution: {execution:?}. Consensus: {consensus}"
    )]
    TerminalBlockHash { execution: Option<H256>, consensus: H256 },
    #[error("Unknown payload")]
    UnknownPayload,
    /// API encountered an internal error.
    #[error(transparent)]
    Internal(reth_interfaces::Error),
}

impl EngineApiError {
    fn code(&self) -> i32 {
        match self {
            Self::UnknownPayload => -38001,
            // Any other server error: https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/errors.go#L79
            _ => -32000,
        }
    }
}

// impl Into<RpcError> for EngineApiError {
//     fn into(self) -> jsonrpsee::core::Error {
//         rpc_err(self.code(), self.to_string(), None)
//     }
// }

impl From<EngineApiError> for RpcError {
    fn from(value: EngineApiError) -> Self {
        rpc_err(value.code(), value.to_string(), None)
    }
}
