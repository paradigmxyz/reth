use alloy_primitives::U256;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForApi, EthApiSpec},
    RpcNodeCore,
};
use reth_storage_api::ProviderTx;

use crate::EthApi;

impl<N, Rpc> EthApiSpec for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Transaction = ProviderTx<N::Provider>;
    type Rpc = Rpc::Network;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &SignersForApi<Self> {
        self.inner.signers()
    }
}
