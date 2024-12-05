//! Miner API extension for OP.

use alloy_primitives::U64;
use jsonrpsee_core::{async_trait, RpcResult};
pub use op_alloy_rpc_jsonrpsee::traits::MinerApiExtServer;
use reth_optimism_payload_builder::config::OpDAConfig;
use tracing::debug;

/// Miner API extension for OP, exposes settings for the data availability configuration via the
/// `miner_` API.
#[derive(Debug, Clone)]
pub struct OpMinerExtApi {
    da_config: OpDAConfig,
}

impl OpMinerExtApi {
    /// Instantiate the miner API extension with the given, sharable data availability
    /// configuration.
    pub const fn new(da_config: OpDAConfig) -> Self {
        Self { da_config }
    }
}

#[async_trait]
impl MinerApiExtServer for OpMinerExtApi {
    /// Handler for `miner_setMaxDASize` RPC method.
    async fn set_max_da_size(&self, max_tx_size: U64, max_block_size: U64) -> RpcResult<()> {
        debug!(target: "rpc", "Setting max DA size: tx={}, block={}", max_tx_size, max_block_size);
        self.da_config.set_max_da_size(max_tx_size.to(), max_block_size.to());
        Ok(())
    }
}
