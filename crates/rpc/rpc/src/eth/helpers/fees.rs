//! Contains RPC handler implementations for fee history.

use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadFee},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, FeeHistoryCache, GasPriceOracle};
use reth_storage_api::ProviderHeader;

use crate::EthApi;

impl<N, Rpc> EthFees for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
}

impl<N, Rpc> LoadFee for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<N::Provider>> {
        self.inner.fee_history_cache()
    }
}
