//! Contains RPC handler implementations for fee history.

use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadBlock, LoadFee},
    RpcNodeCore,
};
use reth_rpc_eth_types::{FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{BlockReader, BlockReaderIdExt, ProviderHeader, StateProviderFactory};

use crate::EthApi;

impl<N> EthFees for EthApi<N>
where
    N: RpcNodeCore<Provider: BlockReader +
        ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = ProviderHeader<N::Provider>>,
        >
    >,
    Self: LoadFee
{
}

impl<N> LoadFee for EthApi<N>
where
    N: RpcNodeCore<
        Provider: BlockReaderIdExt
            + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
            + StateProviderFactory,
    >,
    Self: LoadBlock<Provider = N::Provider>,
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
