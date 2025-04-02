use crate::{
    chainspec::CustomChainSpec,
    engine::{CustomPayloadAttributes, CustomPayloadTypes},
    primitives::CustomNodePrimitives,
};
use alloy_rpc_types_engine::{
    ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus, PayloadStatusEnum,
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, proc_macros::rpc, RpcModule};
use reth_node_api::{AddOnsContext, FullNodeComponents, NodeTypes};
use reth_node_builder::rpc::EngineApiBuilder;
use reth_optimism_node::node::OpStorage;
use reth_rpc_api::IntoEngineApiRpcModule;

#[derive(serde::Deserialize)]
pub struct CustomExecutionPayloadInput {}

#[derive(Clone, serde::Serialize)]
pub struct CustomExecutionPayloadEnvelope {}

#[rpc(server, namespace = "engine")]
pub trait CustomEngineApi {
    #[method(name = "newPayload")]
    async fn new_payload(&self, payload: CustomExecutionPayloadInput) -> RpcResult<PayloadStatus>;

    #[method(name = "forkchoiceUpdated")]
    async fn fork_choice_updated(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<CustomPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "getPayload")]
    async fn get_payload(&self, payload_id: PayloadId)
        -> RpcResult<CustomExecutionPayloadEnvelope>;
}

pub struct CustomEngineApi {}

#[async_trait]
impl CustomEngineApiServer for CustomEngineApi {
    async fn new_payload(&self, _payload: CustomExecutionPayloadInput) -> RpcResult<PayloadStatus> {
        Ok(PayloadStatus::from_status(PayloadStatusEnum::Valid))
    }

    async fn fork_choice_updated(
        &self,
        _fork_choice_state: ForkchoiceState,
        _payload_attributes: Option<CustomPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(ForkchoiceUpdated {
            payload_status: PayloadStatus::from_status(PayloadStatusEnum::Valid),
            payload_id: Some(PayloadId::default()),
        })
    }

    async fn get_payload(
        &self,
        _payload_id: PayloadId,
    ) -> RpcResult<CustomExecutionPayloadEnvelope> {
        Ok(CustomExecutionPayloadEnvelope {})
    }
}

impl IntoEngineApiRpcModule for CustomEngineApi
where
    Self: CustomEngineApiServer,
{
    fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

#[derive(Debug, Default)]
pub struct CustomEngineApiBuilder {}

impl<N> EngineApiBuilder<N> for CustomEngineApiBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Payload = CustomPayloadTypes,
            ChainSpec = CustomChainSpec,
            Primitives = CustomNodePrimitives,
            Storage = OpStorage,
        >,
    >,
{
    type EngineApi = CustomEngineApi;

    async fn build_engine_api(self, _ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        Ok(CustomEngineApi {})
    }
}
