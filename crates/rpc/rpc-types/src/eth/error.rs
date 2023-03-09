//! Commonly used errors for the `eth_` namespace.

/// List of JSON-RPC error codes
#[derive(Debug, Copy, PartialEq, Eq, Clone)]
pub enum EthRpcErrorCode {
    /// Failed to send transaction, See also <https://github.com/MetaMask/eth-rpc-errors/blob/main/src/error-constants.ts>
    TransactionRejected,
    /// Custom geth error code, <https://github.com/vapory-legacy/wiki/blob/master/JSON-RPC-Error-Codes-Improvement-Proposal.md>
    ExecutionError,
    /// <https://eips.ethereum.org/EIPS/eip-1898>
    InvalidInput,
}

impl EthRpcErrorCode {
    /// Returns the error code as `i32`
    pub const fn code(&self) -> i32 {
        match *self {
            EthRpcErrorCode::TransactionRejected => -32003,
            EthRpcErrorCode::ExecutionError => 3,
            EthRpcErrorCode::InvalidInput => -32000,
        }
    }
}
