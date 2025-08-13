use reth_rpc_api::servers::RpcServer; use reth_arbitrum_rpc::nitro::ArbNitroApi;
use reth_arbitrum_rpc::nitro::ArbNitroRpc;
#![allow(unused)]

use alloy_rpc_types_engine::ClientVersionV1;
use reth_chainspec::EthereumHardforks;
use reth_node_api::{AddOnsContext, EngineApiValidator, EngineTypes, FullNodeComponents, NodeTypes};
use reth_node_builder::rpc::{EngineApiBuilder, PayloadValidatorBuilder};
use reth_node_core::version::{CARGO_PKG_VERSION, CLIENT_CODE, VERGEN_GIT_SHA};
use reth_payload_builder::PayloadStore;
use reth_rpc_engine_api::{EngineApi, EngineCapabilities};

use crate::ARB_NAME_CLIENT;
use reth_arbitrum_rpc::engine::ARB_ENGINE_CAPABILITIES;
use reth_arbitrum_payload::ArbExecutionData;

#[derive(Debug, Default, Clone)]
pub struct ArbEngineApiBuilder<EV> {
    engine_validator_builder: EV,
}

impl<EV> ArbEngineApiBuilder<EV> {
    pub fn new(engine_validator_builder: EV) -> Self {
        Self { engine_validator_builder }
    }
}

impl<N, EV> EngineApiBuilder<N> for ArbEngineApiBuilder<EV>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Payload: EngineTypes<ExecutionData = ArbExecutionData>,
        >,
    >,
    EV: PayloadValidatorBuilder<N>,
    EV::Validator: EngineApiValidator<<N::Types as NodeTypes>::Payload>,
{
    type EngineApi = reth_arbitrum_rpc::engine::ArbEngineApi<
        N::Provider,
        <N::Types as NodeTypes>::Payload,
        N::Pool,
        EV::Validator,
        <N::Types as NodeTypes>::ChainSpec,
    >;

    async fn build_engine_api(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        let Self { engine_validator_builder } = self;

        let engine_validator = engine_validator_builder.build(ctx).await?;
        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: ARB_NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };
        let inner = EngineApi::new(
            ctx.node.provider().clone(),
            ctx.config.chain.clone(),
            ctx.beacon_engine_handle.clone(),
            PayloadStore::new(ctx.node.payload_builder_handle().clone()),
            ctx.node.pool().clone(),
            Box::new(ctx.node.task_executor().clone()),
            client,
            EngineCapabilities::new(ARB_ENGINE_CAPABILITIES.iter().copied()),
            engine_validator,
            ctx.config.engine.accept_execution_requests_hash,
        );
        let _ = ctx.rpc_registry.register_methods(ArbNitroRpc::default().into_rpc_module());

        Ok(reth_arbitrum_rpc::engine::ArbEngineApi::new(inner))
    }
}
