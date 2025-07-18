//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, TxEnvFor};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking},
    FromEvmError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt,
};
use reth_storage_api::BlockReader;

impl<Provider, Pool, Network, EvmConfig, Rpc> EthCall
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: EstimateCall<NetworkTypes = Rpc::Network>
        + LoadPendingBlock<NetworkTypes = Rpc::Network>
        + FullEthApiTypes<NetworkTypes = Rpc::Network>
        + RpcNodeCoreExt<Evm = EvmConfig>,
    EvmConfig: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
    Provider: BlockReader,
    Rpc: RpcConvert,
{
}

impl<Provider, Pool, Network, EvmConfig, Rpc> Call
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadState<
            RpcConvert: RpcConvert<TxEnv = TxEnvFor<Self::Evm>, Network = Rpc::Network>,
            NetworkTypes = Rpc::Network,
            Error: FromEvmError<Self::Evm>
                       + From<<Self::RpcConvert as RpcConvert>::Error>
                       + From<ProviderError>,
        > + SpawnBlocking,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm,
    Rpc: RpcConvert,
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

impl<Provider, Pool, Network, EvmConfig, Rpc> EstimateCall
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: Call<NetworkTypes = Rpc::Network>,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm,
    Rpc: RpcConvert,
{
}
