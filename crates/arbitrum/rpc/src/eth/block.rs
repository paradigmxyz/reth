use crate::eth::ArbEthApi;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_storage_api::HeaderProvider;
use crate::error::ArbEthApiError;

impl<N, Rpc> EthBlocks for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}

impl<N, Rpc> LoadBlock for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore<Provider: HeaderProvider>,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}
