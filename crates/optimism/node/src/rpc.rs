//! RPC component builder

pub use reth_optimism_rpc::OpEngineApi;

use op_alloy_rpc_types_engine::OpExecutionData;
use reth_chainspec::EthereumHardforks;
use reth_node_api::{
    AddOnsContext, EngineTypes, FullNodeComponents, NodeTypes, NodeTypesWithEngine,
};
use reth_node_builder::rpc::{BasicEngineApiBuilder, EngineApiBuilder, EngineValidatorBuilder};
use reth_optimism_rpc::engine::OP_CAPABILITIES;

/// Builder for basic [`OpEngineApi`] implementation.
#[derive(Debug, Default)]
pub struct OpEngineApiBuilder<EV> {
    inner: BasicEngineApiBuilder<EV>,
}

impl<N, EV> EngineApiBuilder<N> for OpEngineApiBuilder<EV>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec: EthereumHardforks,
            Engine: EngineTypes<ExecutionData = OpExecutionData>,
        >,
    >,
    EV: EngineValidatorBuilder<N>,
{
    type EngineApi = OpEngineApi<
        N::Provider,
        <N::Types as NodeTypesWithEngine>::Engine,
        N::Pool,
        EV::Validator,
        <N::Types as NodeTypes>::ChainSpec,
    >;

    async fn build_engine_api(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        let inner = self.inner.capabilities(OP_CAPABILITIES).build_engine_api(ctx).await?;

        Ok(OpEngineApi::new(inner))
    }
}
