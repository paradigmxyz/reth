use alloy_primitives::U256;
use reth_chainspec::EthereumHardforks;
use reth_network_api::NetworkInfo;
use reth_provider::{BlockNumReader, ChainSpecProvider, StageCheckpointReader};
use reth_rpc_eth_api::helpers::EthApiSpec;
use reth_transaction_pool::TransactionPool;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthApiSpec for EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + 'static,
    Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
        + BlockNumReader
        + StageCheckpointReader
        + 'static,
    Network: NetworkInfo + 'static,
    EvmConfig: Send + Sync,
{
    fn provider(
        &self,
    ) -> impl ChainSpecProvider<ChainSpec: EthereumHardforks> + BlockNumReader + StageCheckpointReader
    {
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
