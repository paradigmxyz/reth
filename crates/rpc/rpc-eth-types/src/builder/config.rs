//! Configuration for `eth` namespace APIs.

use std::time::Duration;

use crate::{
    EthStateCacheConfig, FeeHistoryCacheConfig, ForwardConfig, GasPriceOracleConfig,
    RPC_DEFAULT_GAS_CAP,
};
use reqwest::Url;
use reth_rpc_server_types::constants::{
    default_max_tracing_requests, DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_BLOCKS_PER_FILTER,
    DEFAULT_MAX_LOGS_PER_RESPONSE, DEFAULT_MAX_SIMULATE_BLOCKS, DEFAULT_MAX_TRACE_FILTER_BLOCKS,
    DEFAULT_PROOF_PERMITS, RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
};
use serde::{Deserialize, Serialize};

/// Default value for stale filter ttl
pub const DEFAULT_STALE_FILTER_TTL: Duration = Duration::from_secs(5 * 60);

/// Config for the locally built pending block
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PendingBlockKind {
    /// Return a pending block with header only, no transactions included
    Empty,
    /// Return null/no pending block
    None,
    /// Return a pending block with all transactions from the mempool (default behavior)
    #[default]
    Full,
}

impl std::str::FromStr for PendingBlockKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "empty" => Ok(Self::Empty),
            "none" => Ok(Self::None),
            "full" => Ok(Self::Full),
            _ => Err(format!(
                "Invalid pending block kind: {s}. Valid options are: empty, none, full"
            )),
        }
    }
}

impl PendingBlockKind {
    /// Returns true if the pending block kind is `None`
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Returns true if the pending block kind is `Empty`
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
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
    /// Maximum number of blocks for `trace_filter` requests.
    pub max_trace_filter_blocks: u64,
    /// Maximum number of blocks that could be scanned per filter request in `eth_getLogs` calls.
    pub max_blocks_per_filter: u64,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    pub max_logs_per_response: usize,
    /// Gas limit for `eth_call` and call tracing RPC methods.
    ///
    /// Defaults to [`RPC_DEFAULT_GAS_CAP`]
    pub rpc_gas_cap: u64,
    /// Max number of blocks for `eth_simulateV1`.
    pub rpc_max_simulate_blocks: u64,
    ///
    /// Sets TTL for stale filters
    pub stale_filter_ttl: Duration,
    /// Settings for the fee history cache
    pub fee_history_cache: FeeHistoryCacheConfig,
    /// The maximum number of getproof calls that can be executed concurrently.
    pub proof_permits: usize,
    /// Maximum batch size for transaction pool insertions.
    pub max_batch_size: usize,
    /// Controls how pending blocks are built when requested via RPC methods
    pub pending_block_kind: PendingBlockKind,
    /// The raw transaction forwarder.
    pub raw_tx_forwarder: ForwardConfig,
    /// Timeout duration for `send_raw_transaction_sync` RPC method.
    pub send_raw_transaction_sync_timeout: Duration,
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
            max_trace_filter_blocks: DEFAULT_MAX_TRACE_FILTER_BLOCKS,
            max_blocks_per_filter: DEFAULT_MAX_BLOCKS_PER_FILTER,
            max_logs_per_response: DEFAULT_MAX_LOGS_PER_RESPONSE,
            rpc_gas_cap: RPC_DEFAULT_GAS_CAP.into(),
            rpc_max_simulate_blocks: DEFAULT_MAX_SIMULATE_BLOCKS,
            stale_filter_ttl: DEFAULT_STALE_FILTER_TTL,
            fee_history_cache: FeeHistoryCacheConfig::default(),
            proof_permits: DEFAULT_PROOF_PERMITS,
            max_batch_size: 1,
            pending_block_kind: PendingBlockKind::Full,
            raw_tx_forwarder: ForwardConfig::default(),
            send_raw_transaction_sync_timeout: RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
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

    /// Configures the maximum number of blocks for `trace_filter` requests
    pub const fn max_trace_filter_blocks(mut self, max_blocks: u64) -> Self {
        self.max_trace_filter_blocks = max_blocks;
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

    /// Configures the maximum gas limit for `eth_call` and call tracing RPC methods
    pub const fn rpc_max_simulate_blocks(mut self, max_blocks: u64) -> Self {
        self.rpc_max_simulate_blocks = max_blocks;
        self
    }

    /// Configures the maximum proof window for historical proof generation.
    pub const fn eth_proof_window(mut self, window: u64) -> Self {
        self.eth_proof_window = window;
        self
    }

    /// Configures the number of getproof requests
    pub const fn proof_permits(mut self, permits: usize) -> Self {
        self.proof_permits = permits;
        self
    }

    /// Configures the maximum batch size for transaction pool insertions
    pub const fn max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = max_batch_size;
        self
    }

    /// Configures the pending block config
    pub const fn pending_block_kind(mut self, pending_block_kind: PendingBlockKind) -> Self {
        self.pending_block_kind = pending_block_kind;
        self
    }

    /// Configures the raw transaction forwarder.
    pub fn raw_tx_forwarder(mut self, tx_forwarder: Option<Url>) -> Self {
        if let Some(tx_forwarder) = tx_forwarder {
            self.raw_tx_forwarder.tx_forwarder = Some(tx_forwarder);
        }
        self
    }

    /// Configures the timeout duration for `send_raw_transaction_sync` RPC method.
    pub const fn send_raw_transaction_sync_timeout(mut self, timeout: Duration) -> Self {
        self.send_raw_transaction_sync_timeout = timeout;
        self
    }
}

/// Config for the filter
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthFilterConfig {
    /// Maximum number of blocks that a filter can scan for logs.
    ///
    /// If `None` then no limit is enforced.
    pub max_blocks_per_filter: Option<u64>,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    ///
    /// If `None` then no limit is enforced.
    pub max_logs_per_response: Option<usize>,
    /// How long a filter remains valid after the last poll.
    ///
    /// A filter is considered stale if it has not been polled for longer than this duration and
    /// will be removed.
    pub stale_filter_ttl: Duration,
}

impl EthFilterConfig {
    /// Sets the maximum number of blocks that a filter can scan for logs.
    pub const fn max_blocks_per_filter(mut self, num: u64) -> Self {
        self.max_blocks_per_filter = Some(num);
        self
    }

    /// Sets the maximum number of logs that can be returned in a single response in `eth_getLogs`
    /// calls.
    pub const fn max_logs_per_response(mut self, num: usize) -> Self {
        self.max_logs_per_response = Some(num);
        self
    }

    /// Sets how long a filter remains valid after the last poll before it will be removed.
    pub const fn stale_filter_ttl(mut self, duration: Duration) -> Self {
        self.stale_filter_ttl = duration;
        self
    }
}

impl Default for EthFilterConfig {
    fn default() -> Self {
        Self {
            max_blocks_per_filter: None,
            max_logs_per_response: None,
            // 5min
            stale_filter_ttl: Duration::from_secs(5 * 60),
        }
    }
}
