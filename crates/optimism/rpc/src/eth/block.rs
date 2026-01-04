//! Loads and formats OP block RPC response.

use crate::{eth::RpcNodeCore, OpEthApi, OpEthApiError};
use reth_optimism_flashblocks::FlashblockPayload;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FromEvmError, RpcConvert,
};

impl<N, Rpc, F> EthBlocks for OpEthApi<N, Rpc, F>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
    F: FlashblockPayload,
{
}

impl<N, Rpc, F> LoadBlock for OpEthApi<N, Rpc, F>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
    F: FlashblockPayload,
{
}
