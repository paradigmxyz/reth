use alloy_consensus::Header;
use reth_evm::ConfigureEvm;
use reth_primitives::EthPrimitives;
use reth_provider::{BlockReader, CanonStateSubscriptions, EvmEnvProvider, StateProviderFactory};
use reth_rpc::{EthFilter, EthPubSub};
use reth_rpc_eth_api::EthApiTypes;
use reth_rpc_eth_types::{
    cache::cache_new_blocks_task, EthApiBuilderCtx, EthConfig, EthStateCache,
};
use reth_tasks::TaskSpawner;

/// Alias for `eth` namespace API builder.
pub type DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> =
    Box<dyn FnOnce(&EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>) -> EthApi>;

/// Handlers for core, filter and pubsub `eth` namespace APIs.
#[derive(Debug, Clone)]
pub struct EthHandlers<Provider, Pool, Network, Events, EthApi: EthApiTypes> {
    /// Main `eth_` request handler
    pub api: EthApi,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Provider, Pool, EthApi>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<Provider, Pool, Events, Network, EthApi::TransactionCompat>,
}

impl<Provider, Pool, Network, Events, EthApi> EthHandlers<Provider, Pool, Network, Events, EthApi>
where
    Provider: StateProviderFactory
        + BlockReader<Block = reth_primitives::Block, Receipt = reth_primitives::Receipt>
        + EvmEnvProvider
        + Clone
        + Unpin
        + 'static,
    Pool: Send + Sync + Clone + 'static,
    Network: Clone + 'static,
    Events: CanonStateSubscriptions<Primitives = EthPrimitives> + Clone + 'static,
    EthApi: EthApiTypes + 'static,
{
    /// Returns a new instance with handlers for `eth` namespace.
    ///
    /// This will spawn all necessary tasks for the handlers.
    #[allow(clippy::too_many_arguments)]
    pub fn bootstrap<EvmConfig, Tasks>(
        provider: Provider,
        pool: Pool,
        network: Network,
        evm_config: EvmConfig,
        config: EthConfig,
        executor: Tasks,
        events: Events,
        eth_api_builder: DynEthApiBuilder<
            Provider,
            Pool,
            EvmConfig,
            Network,
            Tasks,
            Events,
            EthApi,
        >,
    ) -> Self
    where
        EvmConfig: ConfigureEvm<Header = Header>,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let cache = EthStateCache::spawn_with(provider.clone(), config.cache, executor.clone());

        let new_canonical_blocks = events.canonical_state_stream();
        let c = cache.clone();
        executor.spawn_critical(
            "cache canonical blocks task",
            Box::pin(async move {
                cache_new_blocks_task(c, new_canonical_blocks).await;
            }),
        );

        let ctx = EthApiBuilderCtx {
            provider,
            pool,
            network,
            evm_config,
            config,
            executor,
            events,
            cache,
        };

        let api = eth_api_builder(&ctx);

        let filter = EthFilter::new(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.cache.clone(),
            ctx.config.filter_config(),
            Box::new(ctx.executor.clone()),
            api.tx_resp_builder().clone(),
        );

        let pubsub = EthPubSub::with_spawner(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.events.clone(),
            ctx.network.clone(),
            Box::new(ctx.executor.clone()),
            api.tx_resp_builder().clone(),
        );

        Self { api, cache: ctx.cache, filter, pubsub }
    }
}
