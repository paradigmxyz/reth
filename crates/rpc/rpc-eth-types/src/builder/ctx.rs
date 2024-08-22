//! Context required for building `eth` namespace APIs.

use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::ChainSpecProvider;
use reth_storage_api::BlockReaderIdExt;
use reth_tasks::TaskSpawner;

use crate::{
    fee_history::fee_history_cache_new_blocks_task, EthConfig, EthStateCache, FeeHistoryCache,
    GasPriceOracle,
};

/// Context for building the `eth` namespace API.
#[derive(Debug, Clone)]
pub struct EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events> {
    /// Database handle.
    pub provider: Provider,
    /// Mempool handle.
    pub pool: Pool,
    /// Network handle.
    pub network: Network,
    /// EVM configuration.
    pub evm_config: EvmConfig,
    /// RPC config for `eth` namespace.
    pub config: EthConfig,
    /// Runtime handle.
    pub executor: Tasks,
    /// Events handle.
    pub events: Events,
    /// RPC cache handle.
    pub cache: EthStateCache,
}

impl<Provider, Pool, EvmConfig, Network, Tasks, Events>
    EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>
where
    Provider: BlockReaderIdExt + Clone,
{
    /// Returns a new [`FeeHistoryCache`] for the context.
    pub fn new_fee_history_cache(&self) -> FeeHistoryCache
    where
        Provider: ChainSpecProvider + 'static,
        Tasks: TaskSpawner,
        Events: CanonStateSubscriptions,
    {
        FeeHistoryCacheBuilder::build(self)
    }

    /// Returns a new [`GasPriceOracle`] for the context.
    pub fn new_gas_price_oracle(&self) -> GasPriceOracle<Provider> {
        GasPriceOracleBuilder::build(self)
    }
}

/// Builds `eth_` core api component [`GasPriceOracle`], for given context.
#[derive(Debug)]
pub struct GasPriceOracleBuilder;

impl GasPriceOracleBuilder {
    /// Builds a [`GasPriceOracle`], for given context.
    pub fn build<Provider, Pool, EvmConfig, Network, Tasks, Events>(
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> GasPriceOracle<Provider>
    where
        Provider: BlockReaderIdExt + Clone,
    {
        GasPriceOracle::new(ctx.provider.clone(), ctx.config.gas_oracle, ctx.cache.clone())
    }
}

/// Builds `eth_` core api component [`FeeHistoryCache`], for given context.
#[derive(Debug)]
pub struct FeeHistoryCacheBuilder;

impl FeeHistoryCacheBuilder {
    /// Builds a [`FeeHistoryCache`], for given context.
    pub fn build<Provider, Pool, EvmConfig, Network, Tasks, Events>(
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> FeeHistoryCache
    where
        Provider: ChainSpecProvider + BlockReaderIdExt + Clone + 'static,
        Tasks: TaskSpawner,
        Events: CanonStateSubscriptions,
    {
        let fee_history_cache =
            FeeHistoryCache::new(ctx.cache.clone(), ctx.config.fee_history_cache);

        let new_canonical_blocks = ctx.events.canonical_state_stream();
        let fhc = fee_history_cache.clone();
        let provider = ctx.provider.clone();
        ctx.executor.spawn_critical(
            "cache canonical blocks for fee history task",
            Box::pin(async move {
                fee_history_cache_new_blocks_task(fhc, new_canonical_blocks, provider).await;
            }),
        );

        fee_history_cache
    }
}
