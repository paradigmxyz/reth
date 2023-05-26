use reth_rpc::{
    eth::{
        cache::{EthStateCache, EthStateCacheConfig},
        gas_oracle::GasPriceOracleConfig,
    },
    EthApi, EthFilter, EthPubSub,
};
use serde::{Deserialize, Serialize};

/// The default maximum of logs in a single response.
pub(crate) const DEFAULT_MAX_LOGS_IN_RESPONSE: usize = 10_000;

/// The default maximum number of concurrently executed tracing calls
pub(crate) const DEFAULT_MAX_TRACING_REQUESTS: usize = 10;

/// All handlers for the `eth` namespace
#[derive(Debug, Clone)]
pub struct EthHandlers<Client, Pool, Network, Events> {
    /// Main `eth_` request handler
    pub api: EthApi<Client, Pool, Network>,
    /// The async caching layer used by the eth handlers
    pub cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Client, Pool>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: EthPubSub<Client, Pool, Events, Network>,
}

/// Additional config values for the eth namespace
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EthConfig {
    /// Settings for the caching layer
    pub cache: EthStateCacheConfig,
    /// Settings for the gas price oracle
    pub gas_oracle: GasPriceOracleConfig,
    /// The maximum number of tracing calls that can be executed in concurrently.
    pub max_tracing_requests: usize,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    pub max_logs_per_response: usize,
}

impl Default for EthConfig {
    fn default() -> Self {
        Self {
            cache: EthStateCacheConfig::default(),
            gas_oracle: GasPriceOracleConfig::default(),
            max_tracing_requests: DEFAULT_MAX_TRACING_REQUESTS,
            max_logs_per_response: DEFAULT_MAX_LOGS_IN_RESPONSE,
        }
    }
}

impl EthConfig {
    /// Configures the caching layer settings
    pub fn with_state_cache(mut self, cache: EthStateCacheConfig) -> Self {
        self.cache = cache;
        self
    }

    /// Configures the gas price oracle settings
    pub fn with_gpo_config(mut self, gas_oracle_config: GasPriceOracleConfig) -> Self {
        self.gas_oracle = gas_oracle_config;
        self
    }

    /// Configures the maximum number of tracing requests
    pub fn with_max_tracing_requests(mut self, max_requests: usize) -> Self {
        self.max_tracing_requests = max_requests;
        self
    }

    /// Configures the maximum number of logs per response
    pub fn with_max_logs_per_response(mut self, max_logs: usize) -> Self {
        self.max_logs_per_response = max_logs;
        self
    }
}
