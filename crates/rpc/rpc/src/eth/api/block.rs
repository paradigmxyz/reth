//! Contains RPC handler implementations specific to blocks.

use reth_provider::{BlockReaderIdExt, HeaderProvider};

use crate::{
    eth::{
        api::{EthBlocks, LoadBlock},
        cache::EthStateCache,
    },
    EthApi,
};

impl<Provider, Pool, Network, EvmConfig> EthBlocks for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock,
    Provider: HeaderProvider,
{
    #[inline]
    fn provider(&self) -> impl HeaderProvider {
        self.inner.provider()
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadBlock for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
    Provider: BlockReaderIdExt,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
