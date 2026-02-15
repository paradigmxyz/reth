//! Contains RPC handler implementations specific to streams subscriptions.

use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::EthSubscriptions, RpcNodeCore};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

impl<N, Rpc> EthSubscriptions for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
}
