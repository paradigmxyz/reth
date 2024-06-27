//! Builds an RPC receipt response w.r.t. data layout of network.

use crate::{servers::LoadReceipt, EthApi, EthStateCache};

impl<Provider, Pool, Network, EvmConfig> LoadReceipt for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }
}
