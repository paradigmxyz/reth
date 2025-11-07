//! Types for pre-execution

use alloy_primitives::U256;

/// Error codes for pre-execution errors
const UNKNOWN_ERROR_CODE: i32 = 1000;
const INSUFFICIENT_BALANCE_ERROR_CODE: i32 = 1001;
const REVERTED_ERROR_CODE: i32 = 1002;
const CHECK_PRE_ARGS_ERROR_CODE: i32 = 1003;

/// Error information in pre-execution result
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PreExecError {
    /// Error code
    pub code: i32,
    /// Error message
    pub msg: String,
}

impl PreExecError {
    /// Creates an error with the given code and message
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Self { code, msg: msg.into() }
    }

    /// Creates an unknown error
    pub fn unknown(msg: impl Into<String>) -> Self {
        Self::new(UNKNOWN_ERROR_CODE, msg)
    }

    /// Creates an insufficient balance error
    pub fn insufficient_balance(msg: impl Into<String>) -> Self {
        Self::new(INSUFFICIENT_BALANCE_ERROR_CODE, msg)
    }

    /// Creates a reverted error
    pub fn reverted(msg: impl Into<String>) -> Self {
        Self::new(REVERTED_ERROR_CODE, msg)
    }

    /// Creates a check pre-args error
    pub fn check_args(msg: impl Into<String>) -> Self {
        Self::new(CHECK_PRE_ARGS_ERROR_CODE, msg)
    }

    /// Converts this error into a PreExecResult with gas and block number
    pub fn into_result(self, gas_used: u64, block_number: U256) -> PreExecResult {
        PreExecResult::from_error(self, gas_used, block_number)
    }
}

/// Inner transaction information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreExecInnerTx {
    /// Depth of the call
    pub dept: U256,
    /// Internal index
    pub internal_index: U256,
    /// Call type (call, staticcall, delegatecall, etc.)
    pub call_type: String,
    /// Name of the call
    pub name: String,
    /// Trace address
    pub trace_address: String,
    /// Code address
    pub code_address: String,
    /// From address
    pub from: String,
    /// To address
    pub to: String,
    /// Input data
    pub input: String,
    /// Output data
    pub output: String,
    /// Whether the call errored
    pub is_error: bool,
    /// Gas used
    pub gas_used: u64,
    /// Value transferred
    pub value: String,
    /// Value in wei
    pub value_wei: String,
    /// Error message if any
    pub error: String,
    /// Return gas
    pub return_gas: u64,
}

/// State diff for an account
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountStateDiff {
    /// Balance change (before and after)
    pub balance: BalanceChange,
}

/// Balance change information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BalanceChange {
    /// Balance before execution
    pub before: String,
    /// Balance after execution
    pub after: String,
}

/// Result of pre-execution
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreExecResult {
    /// Inner transactions
    pub inner_txs: Option<serde_json::Value>,
    /// Event logs
    pub logs: Option<serde_json::Value>,
    /// State differences
    pub state_diff: Option<serde_json::Value>,
    /// Error information
    pub error: PreExecError,
    /// Gas used
    pub gas_used: u64,
    /// Block number
    pub block_number: U256,
}

impl PreExecResult {
    /// Creates an error result
    pub fn from_error(error: PreExecError, gas_used: u64, block_number: U256) -> Self {
        Self {
            inner_txs: Some(serde_json::Value::Array(vec![])),
            logs: Some(serde_json::Value::Array(vec![])),
            state_diff: Some(serde_json::Value::Object(serde_json::Map::new())),
            error,
            gas_used,
            block_number,
        }
    }

    /// Creates a successful result
    pub fn success(gas_used: u64, block_number: U256) -> Self {
        Self {
            inner_txs: Some(serde_json::Value::Array(vec![])),
            logs: Some(serde_json::Value::Array(vec![])),
            state_diff: Some(serde_json::Value::Object(serde_json::Map::new())),
            error: PreExecError::default(),
            gas_used,
            block_number,
        }
    }
}