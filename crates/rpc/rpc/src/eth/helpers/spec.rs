use alloy_primitives::U256;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_network_api::NetworkInfo;
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, EthSigner},
    RpcNodeCore,
};
use reth_storage_api::{BlockNumReader, BlockReader, ProviderTx, StageCheckpointReader};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthApiSpec for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: RpcNodeCore<Provider = Provider, Network = Network>,
    Provider: BlockReader
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + BlockNumReader
        + StageCheckpointReader,
    Network: NetworkInfo,
{
    type Transaction = ProviderTx<Provider>;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<Self::Transaction>>>> {
        self.inner.signers()
    }
}
