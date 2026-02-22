//! Testing namespace for building a block in a single call.
//!
//! This follows the `testing_buildBlockV1` specification. **Highly sensitive:**
//! testing-only, powerful enough to include arbitrary transactions; must stay
//! disabled by default and never be exposed on public-facing RPC without an
//! explicit operator flag.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, PayloadAttributes};
use jsonrpsee::proc_macros::rpc;

pub use alloy_rpc_types_engine::TESTING_BUILD_BLOCK_V1;

/// Request payload for `testing_buildBlockV1`.
///
/// This is a reth-local duplicate of the alloy type, updated to match the latest
/// spec changes where `transactions` can be `null` (to build from mempool).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestingBuildBlockRequestV1 {
    /// Parent block hash of the block to build.
    pub parent_block_hash: B256,
    /// Payload attributes (V3: timestamp, prevRandao, suggestedFeeRecipient,
    /// withdrawals, parentBeaconBlockRoot).
    pub payload_attributes: PayloadAttributes,
    /// Raw signed transactions to force-include in order.
    ///
    /// - `Some(vec![])`: build an empty block (no transactions)
    /// - `Some(vec![...])`: include exactly these transactions
    /// - `None` / JSON `null`: build a block from the local transaction pool (mempool)
    pub transactions: Option<Vec<Bytes>>,
    /// Optional extra data for the block header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_data: Option<Bytes>,
}

/// Testing RPC interface for building a block in a single call.
///
/// # Enabling
///
/// This namespace is disabled by default for security reasons. To enable it,
/// add `testing` to the `--http.api` flag:
///
/// ```sh
/// reth node --http --http.api eth,testing
/// ```
///
/// **Warning:** Never expose this on public-facing RPC endpoints without proper
/// authentication.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "testing"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "testing"))]
pub trait TestingApi {
    /// Builds a block using the provided parent, payload attributes, and transactions.
    ///
    /// The `transactions` parameter can be:
    /// - An array of raw transactions to include
    /// - An empty array to build an empty block
    /// - `null` to build a block from the mempool
    ///
    /// See <https://github.com/ethereum/execution-apis/pull/747>
    #[method(name = "buildBlockV1")]
    async fn build_block_v1(
        &self,
        parent_block_hash: B256,
        payload_attributes: PayloadAttributes,
        transactions: Option<Vec<Bytes>>,
        extra_data: Option<Bytes>,
    ) -> jsonrpsee::core::RpcResult<ExecutionPayloadEnvelopeV5>;
}
