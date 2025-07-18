//! Contains RPC handler implementations specific to tracing.

use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::Trace, FromEvmError, RpcNodeCore};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

impl<N, Rpc> Trace for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}
