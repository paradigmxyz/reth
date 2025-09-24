//! Contains RPC handler implementations specific to blocks.

use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    FromEvmError, RpcNodeCore,
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock},
};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

impl<N, Rpc> EthBlocks for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
}

impl<N, Rpc> LoadBlock for EthApi<N, Rpc>
where
    Self: LoadPendingBlock,
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
}
