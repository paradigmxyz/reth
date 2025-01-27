use alloy_primitives::U256;
use reth_chainspec::EthereumHardforks;
use reth_network_api::NetworkInfo;
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, ProviderTx, StageCheckpointReader,
};
use reth_rpc_eth_api::{helpers::EthApiSpec, RpcNodeCore};

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
{
    type Transaction = ProviderTx<Provider>;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(
        &self,
    ) -> &parking_lot::RwLock<Vec<Box<dyn reth_rpc_eth_api::helpers::EthSigner<Self::Transaction>>>>
    {
        self.inner.signers()
    }
}
