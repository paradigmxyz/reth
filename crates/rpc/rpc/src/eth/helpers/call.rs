//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, TxEnvFor};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking},
    FromEvmError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::BlockReader;

impl<N, Rpc> EthCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}

impl<N, Rpc> Call for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.max_simulate_blocks()
    }
}

impl<N, Rpc> EstimateCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}
