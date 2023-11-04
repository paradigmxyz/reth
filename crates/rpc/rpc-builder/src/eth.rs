use crate::constants::{
    DEFAULT_MAX_BLOCKS_PER_FILTER, DEFAULT_MAX_LOGS_PER_RESPONSE, DEFAULT_MAX_TRACING_REQUESTS,
};
use reth_rpc::{
    eth::{
        cache::{EthStateCache, EthStateCacheConfig},
        gas_oracle::GasPriceOracleConfig,
        EthFilterConfig, FeeHistoryCacheConfig, RPC_DEFAULT_GAS_CAP,
    },
    BlockingTaskPool, EthApi, EthFilter, EthPubSub,
};
use serde::{Deserialize, Serialize};

/// All handlers for the `eth` namespace
#[derive(Debug, Clone)]
pub struct EthHandlers<Provider, Pool, Network, Events> {
    /// Main `eth_` request handler
    pub api: EthApi<Provider, Pool, Network>,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Provider, Pool>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<Provider, Pool, Events, Network>,
    /// The configured tracing call pool
    pub blocking_task_pool: BlockingTaskPool,
}

/// Additional config values for the eth namespace
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EthConfig {
    /// Settings for the caching layer
    pub cache: EthStateCacheConfig,
    /// Settings for the gas price oracle
    pub gas_oracle: GasPriceOracleConfig,
    /// The maximum number of tracing calls that can be executed in concurrently.
    pub max_tracing_requests: u32,
    /// Maximum number of blocks that could be scanned per filter request in `eth_getLogs` calls.
    pub max_blocks_per_filter: u64,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    pub max_logs_per_response: usize,
    /// Gas limit for `eth_call` and call tracing RPC methods.
    ///
    /// Defaults to [RPC_DEFAULT_GAS_CAP]
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
            max_tracing_requests: DEFAULT_MAX_TRACING_REQUESTS,
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
    pub fn state_cache(mut self, cache: EthStateCacheConfig) -> Self {
        self.cache = cache;
        self
    }

    /// Configures the gas price oracle settings
    pub fn gpo_config(mut self, gas_oracle_config: GasPriceOracleConfig) -> Self {
        self.gas_oracle = gas_oracle_config;
        self
    }

    /// Configures the maximum number of tracing requests
    pub fn max_tracing_requests(mut self, max_requests: u32) -> Self {
        self.max_tracing_requests = max_requests;
        self
    }

    /// Configures the maximum block length to scan per `eth_getLogs` request
    pub fn max_blocks_per_filter(mut self, max_blocks: u64) -> Self {
        self.max_blocks_per_filter = max_blocks;
        self
    }

    /// Configures the maximum number of logs per response
    pub fn max_logs_per_response(mut self, max_logs: usize) -> Self {
        self.max_logs_per_response = max_logs;
        self
    }

    /// Configures the maximum gas limit for `eth_call` and call tracing RPC methods
    pub fn rpc_gas_cap(mut self, rpc_gas_cap: u64) -> Self {
        self.rpc_gas_cap = rpc_gas_cap;
        self
    }
}
