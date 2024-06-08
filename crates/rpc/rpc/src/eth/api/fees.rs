//! Contains RPC handler implementations for fee history.

use reth_provider::{BlockIdReader, BlockReaderIdExt, ChainSpecProvider, HeaderProvider};

use crate::{
    eth::{
        api::{EthFees, LoadFee},
        cache::EthStateCache,
        gas_oracle::GasPriceOracle,
        FeeHistoryCache,
    },
    EthApi,
};

impl<Provider, Pool, Network, EvmConfig> EthFees for EthApi<Provider, Pool, Network, EvmConfig> where
    Self: LoadFee
{
}

impl<Provider, Pool, Network, EvmConfig> LoadFee for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
    Provider: BlockReaderIdExt + HeaderProvider + ChainSpecProvider,
{
    #[inline]
    fn provider(&self) -> impl BlockIdReader + HeaderProvider + ChainSpecProvider {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        &self.inner.cache()
    }

    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<impl BlockReaderIdExt> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        &self.inner.fee_history_cache()
    }
}
