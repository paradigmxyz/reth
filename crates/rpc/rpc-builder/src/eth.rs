use reth_rpc::{
    eth::cache::{EthStateCache, EthStateCacheConfig},
    EthApi, EthFilter, EthPubSub,
};
use serde::{Deserialize, Serialize};

/// All handlers for the `eth` namespace
#[derive(Debug, Clone)]
pub struct EthHandlers<Client, Pool, Network, Events> {
    /// Main `eth_` request handler
    pub api: EthApi<Client, Pool, Network>,
    /// The async caching layer used by the eth handlers
    pub eth_cache: EthStateCache,
    /// Polling based filter handler available on all transports
    pub filter: EthFilter<Client, Pool>,
    /// Handler for subscriptions only available for transports that support it (ws, ipc)
    pub pubsub: Option<EthPubSub<Client, Pool, Events, Network>>,
}

/// Additional config values for the eth namespace
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct EthConfig {
    /// Settings for the caching layer
    pub cache: EthStateCacheConfig,
}
