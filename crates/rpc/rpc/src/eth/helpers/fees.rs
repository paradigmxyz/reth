//! Contains RPC handler implementations for fee history.

use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_eth_api::helpers::{EthFees, LoadBlock, LoadFee};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthFees for EthApi<Provider, Pool, Network, EvmConfig> where
    Self: LoadFee
{
}

impl<Provider, Pool, Network, EvmConfig> LoadFee for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock<Provider = Provider>,
    Provider: BlockReaderIdExt
        + EvmEnvProvider
        + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
        + StateProviderFactory,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.fee_history_cache()
    }
}
