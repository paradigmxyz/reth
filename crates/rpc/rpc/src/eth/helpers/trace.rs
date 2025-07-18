//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{LoadState, Trace},
    FromEvmError,
};
use reth_storage_api::BlockReader;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig, Rpc> Trace
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadState<Error: FromEvmError<Self::Evm>>,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm,
    Rpc: RpcConvert,
{
}
