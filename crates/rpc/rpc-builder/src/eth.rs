use reth_rpc::{EthFilter, EthPubSub};
use reth_rpc_eth_api::EthApiTypes;
use reth_rpc_eth_types::EthConfig;
use reth_tasks::TaskSpawner;

/// Handlers for core, filter and pubsub `eth` namespace APIs.
#[derive(Debug, Clone)]
pub struct EthHandlers<EthApi: EthApiTypes> {
    /// Main `eth_` request handler
    pub api: EthApi,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<EthApi>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<EthApi>,
}

impl<EthApi> EthHandlers<EthApi>
where
    EthApi: EthApiTypes + 'static,
{
    /// Returns a new instance with the additional handlers for the `eth` namespace.
    ///
    /// This will spawn all necessary tasks for the additional handlers.
    #[allow(clippy::too_many_arguments)]
    pub fn bootstrap<Tasks>(config: EthConfig, executor: Tasks, eth_api: EthApi) -> Self
    where
        Tasks: TaskSpawner + Clone + 'static,
    {
        // let cache = EthStateCache::spawn_with(provider.clone(), config.cache, executor.clone());
        //
        // let new_canonical_blocks = provider.canonical_state_stream();
        // let c = cache.clone();
        // executor.spawn_critical(
        //     "cache canonical blocks task",
        //     Box::pin(async move {
        //         cache_new_blocks_task(c, new_canonical_blocks).await;
        //     }),
        // );
        // let ctx = EthApiBuilderCtx { provider, pool, network, evm_config, config, executor, cache
        // };

        let filter =
            EthFilter::new(eth_api.clone(), config.filter_config(), Box::new(executor.clone()));

        let pubsub = EthPubSub::with_spawner(eth_api.clone(), Box::new(executor));

        Self { api: eth_api, filter, pubsub }
    }
}
