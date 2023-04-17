//! Contains RPC handler implementations for fee history.

use crate::{eth::error::EthResult, EthApi};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockId, U64};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::FeeHistory;
use reth_transaction_pool::TransactionPool;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
{
    /// Reports the fee history, for the given amount of blocks, up until the newest block
    /// provided.
    pub(crate) async fn fee_history(
        &self,
        block_count: U64,
        newest_block: BlockId,
        reward_percentiles: Option<Vec<f64>>,
    ) -> EthResult<FeeHistory> {
        todo!()
    }
}
