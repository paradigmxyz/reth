use crate::rpc::{BscEthApi, BscEthApiInner};
use reth::{
    builder::{
        rpc::{EthApiBuilder, EthApiCtx},
        FullNodeComponents,
    },
    rpc::eth::FullEthApiServer,
};
use std::sync::Arc;

/// Builds [`BscEthApi`] for BSC.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BscEthApiBuilder;

impl<N> EthApiBuilder<N> for BscEthApiBuilder
where
    N: FullNodeComponents,
    BscEthApi<N>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
{
    type EthApi = BscEthApi<N>;

    fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> Self::EthApi {
        let eth_api = reth::rpc::eth::EthApiBuilder::new(
            ctx.components.provider().clone(),
            ctx.components.pool().clone(),
            ctx.components.network().clone(),
            ctx.components.evm_config().clone(),
        )
        .eth_cache(ctx.cache)
        .task_spawner(ctx.components.task_executor().clone())
        .gas_cap(ctx.config.rpc_gas_cap.into())
        .max_simulate_blocks(ctx.config.rpc_max_simulate_blocks)
        .eth_proof_window(ctx.config.eth_proof_window)
        .fee_history_cache_config(ctx.config.fee_history_cache)
        .proof_permits(ctx.config.proof_permits)
        .build_inner();

        BscEthApi { inner: Arc::new(BscEthApiInner { eth_api }) }
    }
}
