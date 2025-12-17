//! Contains RPC handler implementations specific to tracing.

use alloy_eips::BlockId;
use alloy_primitives::Address;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthAddresses, Trace},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

impl<N, Rpc> Trace for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
{
}

#[async_trait::async_trait]
impl<N, Rpc> EthAddresses for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
{
    async fn get_addresses_in_block(&self, block_id: BlockId) -> Result<Vec<Address>, EthApiError> {
        Self::collect_addresses_in_block(self, block_id).await
    }
}
