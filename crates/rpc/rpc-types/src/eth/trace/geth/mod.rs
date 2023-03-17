#![allow(missing_docs)]
/// Geth tracing types
use reth_primitives::{Bytes, JsonU256, H256, U256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// re-exported for geth tracing types
pub use ethers_core::types::{GethDebugTracingOptions, GethTraceFrame};

/// Result type for geth style transaction trace
pub type TraceResult = crate::trace::common::TraceResult<serde_json::Value, String>;

/// blockTraceResult represents the results of tracing a single block when an entire chain is being
/// traced. ref <https://github.com/ethereum/go-ethereum/blob/ee530c0d5aa70d2c00ab5691a89ab431b73f8165/eth/tracers/api.go#L218-L222>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTraceResult {
    /// Block number corresponding to the trace task
    pub block: U256,
    /// Block hash corresponding to the trace task
    pub hash: H256,
    /// Trace results produced by the trace task
    pub traces: Vec<TraceResult>,
}

/// Geth Default trace frame
///
/// <https://github.com/ethereum/go-ethereum/blob/a9ef135e2dd53682d106c6a2aede9187026cc1de/eth/tracers/logger/logger.go#L406-L411>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultFrame {
    pub failed: bool,
    pub gas: JsonU256,
    pub return_value: Bytes,
    pub struct_logs: Vec<StructLog>,
}

/// Represents a struct log entry in a trace
///
/// <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L413-L426>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructLog {
    pub depth: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub gas: u64,
    #[serde(rename = "gasCost")]
    pub gas_cost: u64,
    /// ref <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L450-L452>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Vec<String>>,
    pub op: String,
    pub pc: u64,
    #[serde(default, rename = "refund", skip_serializing_if = "Option::is_none")]
    pub refund_counter: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<Vec<U256>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<BTreeMap<H256, H256>>,
}
