use reth_rpc::{
    eth::cache::{EthStateCache, EthStateCacheConfig},
    EthApi, EthFilter, EthPubSub,
};
use serde::{Deserialize, Serialize};

/// The default maximum of logs in a single response.
pub(crate) const DEFAULT_MAX_LOGS_IN_RESPONSE: usize = 2_000;

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
    /// The maximum number of tracing calls that can be executed in concurrently.
    pub max_tracing_requests: usize,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    pub max_logs_per_response: usize,
}

impl Default for EthConfig {
    fn default() -> Self {
        Self {
            cache: EthStateCacheConfig::default(),
            max_tracing_requests: 10,
            max_logs_per_response: DEFAULT_MAX_LOGS_IN_RESPONSE,
        }
    }
}
