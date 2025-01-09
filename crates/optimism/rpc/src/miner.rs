//! Miner API extension for OP.

use alloy_primitives::U64;
use jsonrpsee_core::{async_trait, RpcResult};
pub use op_alloy_rpc_jsonrpsee::traits::MinerApiExtServer;
use reth_node_types::TxTy;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_payload_builder::config::OpDAConfig;
use reth_optimism_primitives::OpPrimitives;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

use tracing::debug;

/// Miner API extension for OP, exposes settings for the data availability configuration via the
/// `miner_` API.
#[derive(Debug, Clone)]
pub struct OpMinerExtApi<Pool> {
    da_config: OpDAConfig,
    tx_pool: Pool,
}

impl<Pool> OpMinerExtApi<Pool> {
    /// Instantiate the miner API extension with the given, sharable data availability
    /// configuration.
    pub const fn new(da_config: OpDAConfig, pool: Pool) -> Self {
        Self { da_config, tx_pool: pool }
    }
}

#[async_trait]
impl<Pool> MinerApiExtServer for OpMinerExtApi<Pool>
where
    Pool: TransactionPool<Transaction: PoolTransaction> + Unpin + 'static
{
    /// Handler for `miner_setMaxDASize` RPC method.
    async fn set_max_da_size(&self, max_tx_size: U64, max_block_size: U64) -> RpcResult<bool> {
        debug!(target: "rpc", "Setting max DA size: tx={}, block={}", max_tx_size, max_block_size);
        self.da_config.set_max_da_size(max_tx_size.to(), max_block_size.to());
        self.tx_pool.update_da_limits(Some(max_tx_size.to()), Some(max_block_size.to()));
        Ok(true)
    }
}
