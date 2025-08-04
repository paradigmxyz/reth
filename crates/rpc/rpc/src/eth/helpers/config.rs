use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::config::EthConfigSpec, RpcNodeCore};

impl<N, Rpc> EthConfigSpec for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}
