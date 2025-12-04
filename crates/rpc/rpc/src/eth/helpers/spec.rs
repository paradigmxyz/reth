use alloy_primitives::U256;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::EthApiSpec, RpcNodeCore};

use crate::EthApi;

impl<N, Rpc> EthApiSpec for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }
}
