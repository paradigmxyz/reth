use reth_evm::ConfigureEvm;
use reth_primitives::NodePrimitives;
use reth_provider::{BlockReader, CanonStateSubscriptions, StateProviderFactory};
use reth_rpc::{EthFilter, EthPubSub};
use reth_rpc_eth_api::EthApiTypes;
use reth_rpc_eth_types::{
    cache::cache_new_blocks_task, EthApiBuilderCtx, EthConfig, EthStateCache,
};
use reth_tasks::TaskSpawner;

/// Alias for `eth` namespace API builder.
pub type DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, EthApi> =
    Box<dyn FnOnce(&EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks>) -> EthApi>;

/// Handlers for core, filter and pubsub `eth` namespace APIs.
#[derive(Debug, Clone)]
pub struct EthHandlers<Provider: BlockReader, EthApi: EthApiTypes> {
    /// Main `eth_` request handler
    pub api: EthApi,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache<Provider::Block, Provider::Receipt>,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<EthApi>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<EthApi>,
}

impl<N, Provider, EthApi> EthHandlers<Provider, EthApi>
where
    N: NodePrimitives,
    Provider: StateProviderFactory
        + BlockReader<Block = N::Block, Receipt = N::Receipt>
        + Clone
        + CanonStateSubscriptions<Primitives = N>
        + Unpin
        + 'static,
    EthApi: EthApiTypes + 'static,
{
    /// Returns a new instance with handlers for `eth` namespace.
    ///
    /// This will spawn all necessary tasks for the handlers.
    #[allow(clippy::too_many_arguments)]
    pub fn bootstrap<EvmConfig, Tasks, Pool, Network>(
        provider: Provider,
        pool: Pool,
        network: Network,
        evm_config: EvmConfig,
        config: EthConfig,
        executor: Tasks,
        eth_api_builder: DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, EthApi>,
    ) -> Self
    where
        EvmConfig: ConfigureEvm<Header = Provider::Header>,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let cache = EthStateCache::spawn_with(provider.clone(), config.cache, executor.clone());

        let new_canonical_blocks = provider.canonical_state_stream();
        let c = cache.clone();
        executor.spawn_critical(
            "cache canonical blocks task",
            Box::pin(async move {
                cache_new_blocks_task(c, new_canonical_blocks).await;
            }),
        );

        let ctx = EthApiBuilderCtx { provider, pool, network, evm_config, config, executor, cache };

        let api = eth_api_builder(&ctx);

        let filter =
            EthFilter::new(api.clone(), ctx.config.filter_config(), Box::new(ctx.executor.clone()));

        let pubsub = EthPubSub::with_spawner(api.clone(), Box::new(ctx.executor.clone()));

        Self { api, cache: ctx.cache, filter, pubsub }
    }
}
