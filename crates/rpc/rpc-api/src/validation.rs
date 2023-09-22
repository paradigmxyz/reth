use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::Bytes;
use reth_rpc_types::{
    engine::ExecutionPayload,
    Message,
    Signature,
};

/// Debug rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "validation"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "validation"))]
pub trait ValidationApi {
    /// Validates a block submitted to the relay
    #[method(name = "validateBuilderSubmissionV1")]
    async fn validate_builder_submission_v1(&self, message: Message, execution_payload: ExecutionPayload, signature: Signature) -> RpcResult<Bytes>;
}
