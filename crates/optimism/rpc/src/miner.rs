//! Miner API extension for OP.

use alloy_primitives::U64;
use jsonrpsee_core::{async_trait, RpcResult};
pub use op_alloy_rpc_jsonrpsee::traits::MinerApiExtServer;
use reth_metrics::{metrics::Gauge, Metrics};
use reth_optimism_payload_builder::config::OpDAConfig;
use tracing::debug;

/// Miner API extension for OP, exposes settings for the data availability configuration via the
/// `miner_` API.
#[derive(Debug, Clone)]
pub struct OpMinerExtApi {
    da_config: OpDAConfig,
    metrics: OpMinerMetrics,
}

impl OpMinerExtApi {
    /// Instantiate the miner API extension with the given, sharable data availability
    /// configuration.
    pub fn new(da_config: OpDAConfig) -> Self {
        Self { da_config, metrics: OpMinerMetrics::default() }
    }
}

#[async_trait]
impl MinerApiExtServer for OpMinerExtApi {
    /// Handler for `miner_setMaxDASize` RPC method.
    async fn set_max_da_size(&self, max_tx_size: U64, max_block_size: U64) -> RpcResult<bool> {
        debug!(target: "rpc", "Setting max DA size: tx={}, block={}", max_tx_size, max_block_size);
        self.da_config.set_max_da_size(max_tx_size.to(), max_block_size.to());

        self.metrics.set_max_da_tx_size(max_tx_size.to());
        self.metrics.set_max_da_block_size(max_block_size.to());

        Ok(true)
    }
}

/// Optimism miner metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "optimism_rpc.miner")]
pub struct OpMinerMetrics {
    /// Max DA tx size set on the miner
    max_da_tx_size: Gauge,
    /// Max DA block size set on the miner
    max_da_block_size: Gauge,
}

impl OpMinerMetrics {
    /// Sets the max DA tx size gauge value
    #[inline]
    pub fn set_max_da_tx_size(&self, size: u64) {
        self.max_da_tx_size.set(size as f64);
    }

    /// Sets the max DA block size gauge value
    #[inline]
    pub fn set_max_da_block_size(&self, size: u64) {
        self.max_da_block_size.set(size as f64);
    }
}
