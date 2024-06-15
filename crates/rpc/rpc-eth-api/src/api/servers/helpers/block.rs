//! Contains RPC handler implementations specific to blocks.

use crate::EthApi;

/// Implements [`EthBlocks`](crate::servers::EthBlocks) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_blocks_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::EthBlocks for $network_api
        where
            Self: $crate::servers::LoadBlock,
            Provider: reth_provider::HeaderProvider,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::HeaderProvider {
                self.inner.provider()
            }
        }
    };
}

/// Implements [`LoadBlock`](crate::servers::LoadBlock) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_block_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::LoadBlock for $network_api
        where
            Self: $crate::servers::LoadPendingBlock + $crate::servers::SpawnBlocking,
            Provider: reth_provider::BlockReaderIdExt,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::BlockReaderIdExt {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::EthStateCache {
                self.inner.cache()
            }
        }
    };
}

eth_blocks_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
load_block_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
