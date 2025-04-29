use super::BscEngineApi;
use crate::node::rpc::engine_api::validator::BscExecutionData;
use reth::{
    api::{AddOnsContext, FullNodeComponents, NodeTypes},
    builder::rpc::EngineApiBuilder,
    primitives::EthereumHardforks,
};
use reth_engine_primitives::EngineTypes;

/// Builder for mocked [`BscEngineApi`] implementation.
#[derive(Debug, Default)]
pub struct BscEngineApiBuilder;

impl<N> EngineApiBuilder<N> for BscEngineApiBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Payload: EngineTypes<ExecutionData = BscExecutionData>,
        >,
    >,
{
    type EngineApi = BscEngineApi;

    async fn build_engine_api(self, _ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        Ok(BscEngineApi::default())
    }
}
