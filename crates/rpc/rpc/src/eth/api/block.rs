//! Contains RPC handler implementations specific to blocks.

use crate::EthApi;

/// Implements [`EthBlocks`](crate::eth::api::EthBlocks) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_blocks_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::EthBlocks for $network_api
        where
            Self: $crate::eth::api::LoadBlock,
            Provider: reth_provider::HeaderProvider,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::HeaderProvider {
                self.inner.provider()
            }
        }
    };
}

/// Implements [`LoadBlock`](crate::eth::api::LoadBlock) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_block_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::LoadBlock for $network_api
        where
            Self: $crate::eth::api::LoadPendingBlock + $crate::eth::api::SpawnBlocking,
            Provider: reth_provider::BlockReaderIdExt,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::BlockReaderIdExt {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::eth::cache::EthStateCache {
                self.inner.cache()
            }
        }
    };
}

eth_blocks_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
load_block_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
