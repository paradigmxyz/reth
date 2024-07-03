use std::{fmt::Debug, time::Duration};

use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_provider::{
    BlockReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, EvmEnvProvider,
    FullRpcProvider, StateProviderFactory,
};
use reth_rpc::{eth::EthFilterConfig, EthApi, EthFilter, EthPubSub};
use reth_rpc_eth_types::{
    cache::cache_new_blocks_task, fee_history::fee_history_cache_new_blocks_task, EthStateCache,
    EthStateCacheConfig, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    GasPriceOracleConfig, RPC_DEFAULT_GAS_CAP,
};
use reth_rpc_server_types::constants::{
    default_max_tracing_requests, DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_BLOCKS_PER_FILTER,
    DEFAULT_MAX_LOGS_PER_RESPONSE,
};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize};

/// Default value for stale filter ttl
const DEFAULT_STALE_FILTER_TTL: Duration = Duration::from_secs(5 * 60);

/// Alias for function that builds the core `eth` namespace API.
pub type EthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> =
    Box<dyn FnOnce(&EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>) -> EthApi>;

/// Handlers for core, filter and pubsub `eth` namespace APIs.
#[derive(Debug, Clone)]
pub struct EthHandlers<Provider, Pool, Network, Events, EthApi> {
    /// Main `eth_` request handler
    pub api: EthApi,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Provider, Pool>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<Provider, Pool, Events, Network>,
}

impl<Provider, Pool, Network, Events, EthApi> EthHandlers<Provider, Pool, Network, Events, EthApi> {
    /// Returns a new [`EthHandlers`] builder.
    #[allow(clippy::too_many_arguments)]
    pub fn builder<EvmConfig, Tasks, EthApiB>(
        provider: Provider,
        pool: Pool,
        network: Network,
        evm_config: EvmConfig,
        config: EthConfig,
        executor: Tasks,
        events: Events,
        eth_api_builder: EthApiB,
    ) -> EthHandlersBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig, EthApi>
    where
        EthApiB: FnOnce(&EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>) -> EthApi
            + 'static,
    {
        EthHandlersBuilder {
            provider,
            pool,
            network,
            evm_config,
            config,
            executor,
            events,
            eth_api_builder: Box::new(eth_api_builder),
        }
    }
}

/// Builds [`EthHandlers`] for core, filter, and pubsub `eth_` apis.
#[allow(missing_debug_implementations)]
pub struct EthHandlersBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig, EthApi> {
    provider: Provider,
    pool: Pool,
    network: Network,
    evm_config: EvmConfig,
    config: EthConfig,
    executor: Tasks,
    events: Events,
    eth_api_builder: EthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>,
}

impl<Provider, Pool, Network, Tasks, Events, EvmConfig, EthApi>
    EthHandlersBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig, EthApi>
where
    Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    Pool: Send + Sync + Clone + 'static,
    EvmConfig: ConfigureEvm,
    Network: Clone,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions + Clone,
{
    /// Returns a new instance with handlers for `eth` namespace.
    pub fn build(self) -> EthHandlers<Provider, Pool, Network, Events, EthApi> {
        let Self { provider, pool, network, evm_config, config, executor, events, eth_api_builder } =
            self;

        let cache = EthStateCache::spawn_with(
            provider.clone(),
            config.cache,
            executor.clone(),
            evm_config.clone(),
        );

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

        let filter = EthFilterApiBuilder::build(&ctx);

        let pubsub = EthPubSubApiBuilder::build(&ctx);

        EthHandlers { api, cache: ctx.cache, filter, pubsub }
    }
}

/// Additional config values for the eth namespace.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EthConfig {
    /// Settings for the caching layer
    pub cache: EthStateCacheConfig,
    /// Settings for the gas price oracle
    pub gas_oracle: GasPriceOracleConfig,
    /// The maximum number of blocks into the past for generating state proofs.
    pub eth_proof_window: u64,
    /// The maximum number of tracing calls that can be executed in concurrently.
    pub max_tracing_requests: usize,
    /// Maximum number of blocks that could be scanned per filter request in `eth_getLogs` calls.
    pub max_blocks_per_filter: u64,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    pub max_logs_per_response: usize,
    /// Gas limit for `eth_call` and call tracing RPC methods.
    ///
    /// Defaults to [`RPC_DEFAULT_GAS_CAP`]
    pub rpc_gas_cap: u64,
    ///
    /// Sets TTL for stale filters
    pub stale_filter_ttl: Duration,
    /// Settings for the fee history cache
    pub fee_history_cache: FeeHistoryCacheConfig,
}

impl EthConfig {
    /// Returns the filter config for the `eth_filter` handler.
    pub fn filter_config(&self) -> EthFilterConfig {
        EthFilterConfig::default()
            .max_blocks_per_filter(self.max_blocks_per_filter)
            .max_logs_per_response(self.max_logs_per_response)
            .stale_filter_ttl(self.stale_filter_ttl)
    }
}

impl Default for EthConfig {
    fn default() -> Self {
        Self {
            cache: EthStateCacheConfig::default(),
            gas_oracle: GasPriceOracleConfig::default(),
            eth_proof_window: DEFAULT_ETH_PROOF_WINDOW,
            max_tracing_requests: default_max_tracing_requests(),
            max_blocks_per_filter: DEFAULT_MAX_BLOCKS_PER_FILTER,
            max_logs_per_response: DEFAULT_MAX_LOGS_PER_RESPONSE,
            rpc_gas_cap: RPC_DEFAULT_GAS_CAP.into(),
            stale_filter_ttl: DEFAULT_STALE_FILTER_TTL,
            fee_history_cache: FeeHistoryCacheConfig::default(),
        }
    }
}

impl EthConfig {
    /// Configures the caching layer settings
    pub const fn state_cache(mut self, cache: EthStateCacheConfig) -> Self {
        self.cache = cache;
        self
    }

    /// Configures the gas price oracle settings
    pub const fn gpo_config(mut self, gas_oracle_config: GasPriceOracleConfig) -> Self {
        self.gas_oracle = gas_oracle_config;
        self
    }

    /// Configures the maximum number of tracing requests
    pub const fn max_tracing_requests(mut self, max_requests: usize) -> Self {
        self.max_tracing_requests = max_requests;
        self
    }

    /// Configures the maximum block length to scan per `eth_getLogs` request
    pub const fn max_blocks_per_filter(mut self, max_blocks: u64) -> Self {
        self.max_blocks_per_filter = max_blocks;
        self
    }

    /// Configures the maximum number of logs per response
    pub const fn max_logs_per_response(mut self, max_logs: usize) -> Self {
        self.max_logs_per_response = max_logs;
        self
    }

    /// Configures the maximum gas limit for `eth_call` and call tracing RPC methods
    pub const fn rpc_gas_cap(mut self, rpc_gas_cap: u64) -> Self {
        self.rpc_gas_cap = rpc_gas_cap;
        self
    }

    /// Configures the maximum proof window for historical proof generation.
    pub const fn eth_proof_window(mut self, window: u64) -> Self {
        self.eth_proof_window = window;
        self
    }
}

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

/// Ethereum layer one `eth` RPC server builder.
#[derive(Default, Debug, Clone, Copy)]
pub struct EthApiBuild;

impl EthApiBuild {
    /// Builds the [`EthApiServer`](reth_rpc_eth_api::EthApiServer), for given context.
    pub fn build<Provider, Pool, EvmConfig, Network, Tasks, Events>(
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthApi<Provider, Pool, Network, EvmConfig>
    where
        Provider: FullRpcProvider,
        Pool: TransactionPool,
        Network: NetworkInfo + Clone,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions,
        EvmConfig: ConfigureEvm,
    {
        let gas_oracle = GasPriceOracleBuilder::build(ctx);
        let fee_history_cache = FeeHistoryCacheBuilder::build(ctx);

        EthApi::with_spawner(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.network.clone(),
            ctx.cache.clone(),
            gas_oracle,
            ctx.config.rpc_gas_cap,
            ctx.config.eth_proof_window,
            Box::new(ctx.executor.clone()),
            BlockingTaskPool::build().expect("failed to build blocking task pool"),
            fee_history_cache,
            ctx.evm_config.clone(),
            None,
        )
    }
}

/// Builds the `eth_` namespace API [`EthFilterApiServer`](reth_rpc_eth_api::EthFilterApiServer).
#[derive(Debug)]
pub struct EthFilterApiBuilder;

impl EthFilterApiBuilder {
    /// Builds the [`EthFilterApiServer`](reth_rpc_eth_api::EthFilterApiServer), for given context.
    pub fn build<Provider, Pool, EvmConfig, Network, Tasks, Events>(
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthFilter<Provider, Pool>
    where
        Provider: Send + Sync + Clone + 'static,
        Pool: Send + Sync + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        EthFilter::new(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.cache.clone(),
            ctx.config.filter_config(),
            Box::new(ctx.executor.clone()),
        )
    }
}

/// Builds the `eth_` namespace API [`EthPubSubApiServer`](reth_rpc_eth_api::EthFilterApiServer).
#[derive(Debug)]
pub struct EthPubSubApiBuilder;

impl EthPubSubApiBuilder {
    /// Builds the [`EthPubSubApiServer`](reth_rpc_eth_api::EthPubSubApiServer), for given context.
    pub fn build<Provider, Pool, EvmConfig, Network, Tasks, Events>(
        ctx: &EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthPubSub<Provider, Pool, Events, Network>
    where
        Provider: Clone,
        Pool: Clone,
        Events: Clone,
        Network: Clone,
        Tasks: TaskSpawner + Clone + 'static,
    {
        EthPubSub::with_spawner(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.events.clone(),
            ctx.network.clone(),
            Box::new(ctx.executor.clone()),
        )
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
