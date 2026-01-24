//! Testing namespace for building a block in a single call.
//!
//! This follows the `testing_buildBlockV1` specification. **Highly sensitive:**
//! testing-only, powerful enough to include arbitrary transactions; must stay
//! disabled by default and never be exposed on public-facing RPC without an
//! explicit operator flag.

use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV5;
use jsonrpsee::proc_macros::rpc;

pub use alloy_rpc_types_engine::{TestingBuildBlockRequestV1, TESTING_BUILD_BLOCK_V1};

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
}
