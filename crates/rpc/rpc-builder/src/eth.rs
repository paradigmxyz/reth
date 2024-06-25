use crate::RpcModuleConfig;
use reth_evm::ConfigureEvm;
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    EvmEnvProvider, StateProviderFactory,
};
use reth_rpc::{
    eth::{
        cache::{cache_new_blocks_task, EthStateCache, EthStateCacheConfig},
        fee_history_cache_new_blocks_task,
        gas_oracle::{GasPriceOracle, GasPriceOracleConfig},
        traits::RawTransactionForwarder,
        EthFilterConfig, FeeHistoryCache, FeeHistoryCacheConfig, RPC_DEFAULT_GAS_CAP,
    },
    EthApi, EthFilter, EthPubSub,
};
use reth_rpc_server_types::constants::{
    default_max_tracing_requests, DEFAULT_MAX_BLOCKS_PER_FILTER, DEFAULT_MAX_LOGS_PER_RESPONSE,
};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// All handlers for the `eth` namespace
#[derive(Debug, Clone)]
pub struct EthHandlers<Provider, Pool, Network, Events, EvmConfig> {
    /// Main `eth_` request handler
    pub api: EthApi<Provider, Pool, Network, EvmConfig>,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Provider, Pool>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<Provider, Pool, Events, Network>,
    /// The configured tracing call pool
    pub blocking_task_pool: BlockingTaskPool,
}

/// Configuration for `EthHandlersBuilder`
#[derive(Clone, Debug)]
pub(crate) struct EthHandlersConfig<Provider, Pool, Network, Tasks, Events, EvmConfig> {
    /// The provider for blockchain data, responsible for reading blocks, accounts, state, etc.
    pub(crate) provider: Provider,
    /// The transaction pool for managing pending transactions.
    pub(crate) pool: Pool,
    /// The network information, handling peer connections and network state.
    pub(crate) network: Network,
    /// The task executor for spawning asynchronous tasks.
    pub(crate) executor: Tasks,
    /// The event subscriptions for canonical state changes.
    pub(crate) events: Events,
    /// The EVM configuration for Ethereum Virtual Machine settings.
    pub(crate) evm_config: EvmConfig,
    /// An optional forwarder for raw transactions.
    pub(crate) eth_raw_transaction_forwarder: Option<Arc<dyn RawTransactionForwarder>>,
}

/// Represents the builder for the `EthHandlers` struct, used to configure and create instances of
/// `EthHandlers`.
#[derive(Debug, Clone)]
pub(crate) struct EthHandlersBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig> {
    eth_handlers_config: EthHandlersConfig<Provider, Pool, Network, Tasks, Events, EvmConfig>,
    /// Configuration for the RPC module
    rpc_config: RpcModuleConfig,
}

impl<Provider, Pool, Network, Tasks, Events, EvmConfig>
    EthHandlersBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig>
where
    Provider: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    EvmConfig: ConfigureEvm + 'static,
{
    /// Creates a new `EthHandlersBuilder` with the provided components.
    pub(crate) const fn new(
        eth_handlers_config: EthHandlersConfig<Provider, Pool, Network, Tasks, Events, EvmConfig>,
        rpc_config: RpcModuleConfig,
    ) -> Self {
        Self { eth_handlers_config, rpc_config }
    }

    /// Builds and returns an `EthHandlers` instance.
    pub(crate) fn build(self) -> EthHandlers<Provider, Pool, Network, Events, EvmConfig> {
        // Initialize the cache
        let cache = self.init_cache();

        // Initialize the fee history cache
        let fee_history_cache = self.init_fee_history_cache(&cache);

        // Spawn background tasks for cache
        self.spawn_cache_tasks(&cache, &fee_history_cache);

        // Initialize the gas oracle
        let gas_oracle = self.init_gas_oracle(&cache);

        // Initialize the blocking task pool
        let blocking_task_pool = self.init_blocking_task_pool();

        // Initialize the Eth API
        let api = self.init_api(&cache, gas_oracle, &fee_history_cache, &blocking_task_pool);

        // Initialize the filter
        let filter = self.init_filter(&cache);

        // Initialize the pubsub
        let pubsub = self.init_pubsub();

        EthHandlers { api, cache, filter, pubsub, blocking_task_pool }
    }

    /// Initializes the `EthStateCache`.
    fn init_cache(&self) -> EthStateCache {
        EthStateCache::spawn_with(
            self.eth_handlers_config.provider.clone(),
            self.rpc_config.eth.cache.clone(),
            self.eth_handlers_config.executor.clone(),
            self.eth_handlers_config.evm_config.clone(),
        )
    }

    /// Initializes the `FeeHistoryCache`.
    fn init_fee_history_cache(&self, cache: &EthStateCache) -> FeeHistoryCache {
        FeeHistoryCache::new(cache.clone(), self.rpc_config.eth.fee_history_cache.clone())
    }

    /// Spawns background tasks for updating caches.
    fn spawn_cache_tasks(&self, cache: &EthStateCache, fee_history_cache: &FeeHistoryCache) {
        // Get the stream of new canonical blocks
        let new_canonical_blocks = self.eth_handlers_config.events.canonical_state_stream();

        // Clone the cache for the task
        let cache_clone = cache.clone();

        // Spawn a critical task to update the cache with new blocks
        self.eth_handlers_config.executor.spawn_critical(
            "cache canonical blocks task",
            Box::pin(async move {
                cache_new_blocks_task(cache_clone, new_canonical_blocks).await;
            }),
        );

        // Get another stream of new canonical blocks
        let new_canonical_blocks = self.eth_handlers_config.events.canonical_state_stream();

        // Clone the fee history cache for the task
        let fhc_clone = fee_history_cache.clone();

        // Clone the provider for the task
        let provider_clone = self.eth_handlers_config.provider.clone();

        // Spawn a critical task to update the fee history cache with new blocks
        self.eth_handlers_config.executor.spawn_critical(
            "cache canonical blocks for fee history task",
            Box::pin(async move {
                fee_history_cache_new_blocks_task(fhc_clone, new_canonical_blocks, provider_clone)
                    .await;
            }),
        );
    }

    /// Initializes the `GasPriceOracle`.
    fn init_gas_oracle(&self, cache: &EthStateCache) -> GasPriceOracle<Provider> {
        GasPriceOracle::new(
            self.eth_handlers_config.provider.clone(),
            self.rpc_config.eth.gas_oracle.clone(),
            cache.clone(),
        )
    }

    /// Initializes the `BlockingTaskPool`.
    fn init_blocking_task_pool(&self) -> BlockingTaskPool {
        BlockingTaskPool::build().expect("failed to build tracing pool")
    }

    /// Initializes the `EthApi`.
    fn init_api(
        &self,
        cache: &EthStateCache,
        gas_oracle: GasPriceOracle<Provider>,
        fee_history_cache: &FeeHistoryCache,
        blocking_task_pool: &BlockingTaskPool,
    ) -> EthApi<Provider, Pool, Network, EvmConfig> {
        EthApi::with_spawner(
            self.eth_handlers_config.provider.clone(),
            self.eth_handlers_config.pool.clone(),
            self.eth_handlers_config.network.clone(),
            cache.clone(),
            gas_oracle,
            self.rpc_config.eth.rpc_gas_cap,
            Box::new(self.eth_handlers_config.executor.clone()),
            blocking_task_pool.clone(),
            fee_history_cache.clone(),
            self.eth_handlers_config.evm_config.clone(),
            self.eth_handlers_config.eth_raw_transaction_forwarder.clone(),
        )
    }

    /// Initializes the `EthFilter`.
    fn init_filter(&self, cache: &EthStateCache) -> EthFilter<Provider, Pool> {
        EthFilter::new(
            self.eth_handlers_config.provider.clone(),
            self.eth_handlers_config.pool.clone(),
            cache.clone(),
            self.rpc_config.eth.filter_config(),
            Box::new(self.eth_handlers_config.executor.clone()),
        )
    }

    /// Initializes the `EthPubSub`.
    fn init_pubsub(&self) -> EthPubSub<Provider, Pool, Events, Network> {
        EthPubSub::with_spawner(
            self.eth_handlers_config.provider.clone(),
            self.eth_handlers_config.pool.clone(),
            self.eth_handlers_config.events.clone(),
            self.eth_handlers_config.network.clone(),
            Box::new(self.eth_handlers_config.executor.clone()),
        )
    }
}

/// Additional config values for the eth namespace.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EthConfig {
    /// Settings for the caching layer
    pub cache: EthStateCacheConfig,
    /// Settings for the gas price oracle
    pub gas_oracle: GasPriceOracleConfig,
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
    pub stale_filter_ttl: std::time::Duration,
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

/// Default value for stale filter ttl
const DEFAULT_STALE_FILTER_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl Default for EthConfig {
    fn default() -> Self {
        Self {
            cache: EthStateCacheConfig::default(),
            gas_oracle: GasPriceOracleConfig::default(),
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
}
