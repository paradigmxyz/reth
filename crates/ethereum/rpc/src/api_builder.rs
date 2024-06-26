use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_provider::{CanonStateSubscriptions, FullRpcProvider};
use reth_rpc_builder::{
    EthApiBuilder, EthApiBuilderCtx, FeeHistoryCacheBuilder, GasPriceOracleBuilder,
};
use reth_rpc_eth_api::EthApi;
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};
use reth_transaction_pool::TransactionPool;

/// Ethereum layer one `eth` RPC server builder.
#[derive(Default, Debug, Clone, Copy)]
pub struct EthApiBuild;

impl<Provider, Pool, EvmConfig, Network, Tasks, Events>
    EthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events> for EthApiBuild
where
    Provider: FullRpcProvider,
    Pool: TransactionPool,
    Network: NetworkInfo + Clone,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions,
    EvmConfig: ConfigureEvm,
{
    type Server = EthApi<Provider, Pool, Network, EvmConfig>;

    fn build(
        &self,
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> Self::Server {
        let gas_oracle = GasPriceOracleBuilder::build(ctx);
        let fee_history_cache = FeeHistoryCacheBuilder::build(ctx);

        EthApi::with_spawner(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.network.clone(),
            ctx.cache.clone(),
            gas_oracle,
            ctx.config.rpc_gas_cap,
            Box::new(ctx.executor.clone()),
            BlockingTaskPool::build().expect("failed to build blocking task pool"),
            fee_history_cache,
            ctx.evm_config.clone(),
            None,
        )
    }
}
