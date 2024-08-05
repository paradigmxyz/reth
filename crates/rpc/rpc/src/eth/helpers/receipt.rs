//! Builds an RPC receipt response w.r.t. data layout of network.

use reth_rpc_eth_api::helpers::LoadReceipt;
use reth_rpc_eth_types::EthStateCache;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadReceipt for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
