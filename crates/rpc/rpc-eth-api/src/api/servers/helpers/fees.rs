//! Contains RPC handler implementations for fee history.

use crate::EthApi;

/// Implements [`EthFees`](crate::servers::EthFees) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_fees_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::EthFees for $network_api where
            Self: $crate::servers::LoadFee
        {
        }
    };
}

/// Implements [`LoadFee`](crate::servers::LoadFee) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_fee_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::LoadFee for $network_api
        where
            Self: $crate::servers::LoadBlock,
            Provider: reth_provider::BlockReaderIdExt
                + reth_provider::HeaderProvider
                + reth_provider::ChainSpecProvider,
        {
            #[inline]
            fn provider(
                &self,
            ) -> impl reth_provider::BlockIdReader
                   + reth_provider::HeaderProvider
                   + reth_provider::ChainSpecProvider {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::EthStateCache {
                self.inner.cache()
            }

            #[inline]
            fn gas_oracle(&self) -> &$crate::GasPriceOracle<impl reth_provider::BlockReaderIdExt> {
                self.inner.gas_oracle()
            }

            #[inline]
            fn fee_history_cache(&self) -> &$crate::FeeHistoryCache {
                self.inner.fee_history_cache()
            }
        }
    };
}

eth_fees_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
load_fee_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
