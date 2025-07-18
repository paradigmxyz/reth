//! Contains RPC handler implementations specific to blocks.

use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, SpawnBlocking},
    RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::BlockReader;

use crate::EthApi;

impl<N, Rpc> EthBlocks for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}

impl<N, Rpc> LoadBlock for EthApi<N, Rpc>
where
    Self: LoadPendingBlock,
    N: RpcNodeCore,
    Rpc: RpcConvert,
{
}
