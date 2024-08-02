use reth_network_api::NetworkInfo;
use reth_primitives::U256;
use reth_provider::{BlockNumReader, ChainSpecProvider};
use reth_rpc_eth_api::helpers::EthApiSpec;
use reth_transaction_pool::TransactionPool;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthApiSpec for EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + 'static,
    Provider: BlockNumReader + ChainSpecProvider + 'static,
    Network: NetworkInfo + 'static,
    EvmConfig: Send + Sync,
{
    fn provider(&self) -> impl ChainSpecProvider + BlockNumReader {
        self.inner.provider()
    }

    fn network(&self) -> impl NetworkInfo {
        self.inner.network()
    }

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn reth_rpc_eth_api::helpers::EthSigner>>> {
        self.inner.signers()
    }
}
