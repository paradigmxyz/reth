//! Contains RPC handler implementations for fee history.

use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadBlock, LoadFee},
    RpcNodeCore,
};
use reth_rpc_eth_types::{FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{BlockReader, BlockReaderIdExt, ProviderHeader, StateProviderFactory};

use crate::EthApi;

impl<N, Rpc> EthFees for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}

impl<N, Rpc> LoadFee for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<Provider>> {
        self.inner.fee_history_cache()
    }
}
