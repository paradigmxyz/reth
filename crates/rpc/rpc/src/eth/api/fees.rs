//! Contains RPC handler implementations for fee history.

use crate::EthApi;

/// Implements [`EthFees`](crate::eth::api::EthFees) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_fees_impl {
    ($network_api:ty, $(<$($generic:ident,)+>)*) => {
        impl$(<$($generic,)+>)* $crate::eth::api::EthFees
            for $network_api
        where
            Self: $crate::eth::api::LoadFee,
        {
        }
    };
}

/// Implements [`LoadFee`](crate::eth::api::LoadFee) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_fee_impl {
    ($network_api:ty, $(<$($generic:ident,)+>)*) => {
        impl$(<$($generic,)+>)* $crate::eth::api::LoadFee
            for $network_api
        where
            Self: $crate::eth::api::LoadBlock,
            Provider: reth_provider::BlockReaderIdExt + reth_provider::HeaderProvider + reth_provider::ChainSpecProvider,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::BlockIdReader + reth_provider::HeaderProvider + reth_provider::ChainSpecProvider {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::eth::cache::EthStateCache {
                self.inner.cache()
            }

            #[inline]
            fn gas_oracle(&self) -> &$crate::eth::gas_oracle::GasPriceOracle<impl reth_provider::BlockReaderIdExt> {
                self.inner.gas_oracle()
            }

            #[inline]
            fn fee_history_cache(&self) -> &$crate::eth::FeeHistoryCache {
                self.inner.fee_history_cache()
            }
        }
    };
}

eth_fees_impl!(EthApi<Provider, Pool, Network, EvmConfig>, <Provider, Pool, Network, EvmConfig,>);
load_fee_impl!(EthApi<Provider, Pool, Network, EvmConfig>, <Provider, Pool, Network, EvmConfig,>);
