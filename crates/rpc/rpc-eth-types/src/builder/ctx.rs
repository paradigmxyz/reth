//! Context required for building `eth` namespace APIs.

use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::ChainSpecProvider;
use reth_primitives::NodePrimitives;
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
        Events: CanonStateSubscriptions<
            Primitives: NodePrimitives<
                Block = reth_primitives::Block,
                Receipt = reth_primitives::Receipt,
            >,
        >,
    {
        let fee_history_cache =
            FeeHistoryCache::new(self.cache.clone(), self.config.fee_history_cache);

        let new_canonical_blocks = self.events.canonical_state_stream();
        let fhc = fee_history_cache.clone();
        let provider = self.provider.clone();
        self.executor.spawn_critical(
            "cache canonical blocks for fee history task",
            Box::pin(async move {
                fee_history_cache_new_blocks_task(fhc, new_canonical_blocks, provider).await;
            }),
        );

        fee_history_cache
    }

    /// Returns a new [`GasPriceOracle`] for the context.
    pub fn new_gas_price_oracle(&self) -> GasPriceOracle<Provider> {
        GasPriceOracle::new(self.provider.clone(), self.config.gas_oracle, self.cache.clone())
    }
}
