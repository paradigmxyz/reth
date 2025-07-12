//! Contains RPC handler implementations for fee history.

use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_rpc_eth_api::helpers::{EthFees, LoadBlock, LoadFee};
use reth_rpc_eth_types::{FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{BlockReader, BlockReaderIdExt, ProviderHeader, StateProviderFactory};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig, Rpc> EthFees
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadFee<
        Provider: ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = ProviderHeader<Self::Provider>>,
        >,
    >,
    Provider: BlockReader,
    Rpc: alloy_network::Network,
{
}

impl<Provider, Pool, Network, EvmConfig, Rpc> LoadFee
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadBlock<Provider = Provider>,
    Provider: BlockReaderIdExt
        + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
        + StateProviderFactory,
    Rpc: alloy_network::Network,
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
