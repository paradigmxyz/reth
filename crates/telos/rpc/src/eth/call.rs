use reth_evm::ConfigureEvm;
use reth_node_api::FullNodeComponents;
use reth_rpc_eth_api::{
    helpers::{Call, EthCall, LoadState, SpawnBlocking},
};
use crate::error::TelosEthApiError;
use crate::eth::TelosEthApi;

impl<N> EthCall for TelosEthApi<N>
where
    Self: Call,
    N: FullNodeComponents,
{
}

impl<N> Call for TelosEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    Self::Error: From<TelosEthApiError>,
    N: FullNodeComponents,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }

}
