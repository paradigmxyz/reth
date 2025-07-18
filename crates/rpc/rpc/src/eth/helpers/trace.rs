//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{LoadState, Trace},
    FromEvmError,
};
use reth_storage_api::BlockReader;

use crate::EthApi;

impl<N, Rpc> Trace for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    RpcConvert: RpcConvert<Primitives = N::Primitives>,
{
}
