use alloy_network::Ethereum;
use alloy_primitives::U256;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_network_api::NetworkInfo;
use reth_rpc_convert::RpcTypes;
use reth_rpc_eth_api::{
    helpers::{spec::Signers, EthApiSpec},
    RpcNodeCore,
};
use reth_storage_api::{BlockNumReader, BlockReader, ProviderTx, StageCheckpointReader};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthApiSpec for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: RpcNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
    Provider: BlockReader,
    Network: RpcTypes,
{
    type Transaction = ProviderTx<Provider>;
    type Rpc = Ethereum;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &Signers<Self> {
        self.inner.signers()
    }
}
