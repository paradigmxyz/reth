use crate::node::rpc::engine_api::validator::BscExecutionData;
use alloy_rpc_types_engine::ClientVersionV1;
use reth::{
    api::{AddOnsContext, FullNodeComponents, NodeTypes},
    builder::rpc::{EngineApiBuilder, EngineValidatorBuilder},
    payload::PayloadStore,
    primitives::EthereumHardforks,
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_engine_primitives::EngineTypes;
use reth_rpc_engine_api::{EngineApi, EngineCapabilities};

use super::BscEngineApi;

/// Builder for basic [`EngineApi`] implementation.
#[derive(Debug, Default)]
pub struct BscEngineApiBuilder<EV> {
    engine_validator_builder: EV,
}

impl<N, EV> EngineApiBuilder<N> for BscEngineApiBuilder<EV>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Payload: EngineTypes<ExecutionData = BscExecutionData>,
        >,
    >,
    EV: EngineValidatorBuilder<N>,
{
    type EngineApi = BscEngineApi<
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
            name: NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };
        Ok(BscEngineApi::new(EngineApi::new(
            ctx.node.provider().clone(),
            ctx.config.chain.clone(),
            ctx.beacon_engine_handle.clone(),
            PayloadStore::new(ctx.node.payload_builder_handle().clone()),
            ctx.node.pool().clone(),
            Box::new(ctx.node.task_executor().clone()),
            client,
            EngineCapabilities::default(),
            engine_validator,
            ctx.config.engine.accept_execution_requests_hash,
        )))
    }
}
