//! Testing namespace for building a block in a single call.
//!
//! This follows the `testing_buildBlockV1` specification. **Highly sensitive:**
//! testing-only, powerful enough to include arbitrary transactions; must stay
//! disabled by default and never be exposed on public-facing RPC without an
//! explicit operator flag.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, PayloadAttributes};
use jsonrpsee::proc_macros::rpc;

pub use alloy_rpc_types_engine::{TestingBuildBlockRequestV1, TESTING_BUILD_BLOCK_V1};

/// Capability string for `testing_commitBlockV1`.
pub const TESTING_COMMIT_BLOCK_V1: &str = "testing_commitBlockV1";

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
    /// See <https://github.com/marcindsobczak/execution-apis/blob/main/src/testing/testing_buildBlockV1.md>
    #[method(name = "buildBlockV1")]
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> jsonrpsee::core::RpcResult<ExecutionPayloadEnvelopeV5>;

    /// Builds a block on top of the current canonical head, inserts it, and makes it canonical.
    ///
    /// See <https://github.com/ethereum/execution-apis/pull/801>
    #[method(name = "commitBlockV1")]
    async fn commit_block_v1(
        &self,
        payload_attributes: PayloadAttributes,
        transactions: Option<Vec<Bytes>>,
        extra_data: Option<Bytes>,
    ) -> jsonrpsee::core::RpcResult<B256>;
}
