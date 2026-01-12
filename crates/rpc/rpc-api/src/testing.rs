//! Testing namespace for building a block in a single call.
//!
//! This follows the `testing_buildBlockV1` specification. **Highly sensitive:**
//! testing-only, powerful enough to include arbitrary transactions; must stay
//! disabled by default and never be exposed on public-facing RPC without an
//! explicit operator flag.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV5, PayloadAttributes as EthPayloadAttributes,
};
use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

/// Capability string for `testing_buildBlockV1`.
pub const TESTING_BUILD_BLOCK_V1: &str = "testing_buildBlockV1";

/// Request payload for `testing_buildBlockV1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestingBuildBlockRequestV1 {
    /// Parent block hash of the block to build.
    pub parent_block_hash: B256,
    /// Payload attributes (Cancun version).
    pub payload_attributes: EthPayloadAttributes,
    /// Raw signed transactions to force-include in order.
    pub transactions: Vec<Bytes>,
    /// Optional extra data for the block header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_data: Option<Bytes>,
}

/// Testing RPC interface for building a block in a single call.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "testing"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "testing"))]
pub trait TestingApi {
    /// Builds a block using the provided parent, payload attributes, and transactions.
    ///
    /// See <https://github.com/marcindsobczak/execution-apis/blob/main/src/testing/testing_buildBlockV1.md>
    #[method(name = "buildBlockV1")]
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> jsonrpsee::core::RpcResult<ExecutionPayloadEnvelopeV5>;

    /// Builds a block using the provided parent, payload attributes, and transactions.
    ///
    /// Like testing_buildBlockV1 but skips invalid transactions.
    #[method(name = "packBlock")]
    async fn pack_block(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> jsonrpsee::core::RpcResult<ExecutionPayloadEnvelopeV5>;
}
