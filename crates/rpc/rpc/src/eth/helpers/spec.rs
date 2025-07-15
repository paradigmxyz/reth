use alloy_network::Ethereum;
use alloy_primitives::U256;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_network_api::NetworkInfo;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForApi, EthApiSpec},
    RpcNodeCore,
};
use reth_storage_api::{BlockNumReader, BlockReader, ProviderTx, StageCheckpointReader};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthApiSpec
    for EthApi<Provider, Pool, Network, EvmConfig, Ethereum>
where
    Self: RpcNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
    Provider: BlockReader,
{
    type Transaction = ProviderTx<Provider>;
    type Rpc = Ethereum;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &SignersForApi<Self> {
        self.inner.signers()
    }
}
