//! Builds an RPC receipt response w.r.t. data layout of network.

/// Implements [`LoadReceipt`](crate::servers::LoadReceipt) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_receipt_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::LoadReceipt for $network_api
        where
            Self: Send + Sync,
        {
            #[inline]
            fn cache(&self) -> &$crate::EthStateCache {
                &self.inner.eth_cache
            }
        }
    };
}

#[cfg(not(feature = "optimism"))]
load_receipt_impl!(crate::EthApi<Provider, Pool, Network, EvmConfig>);
