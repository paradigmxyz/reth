//! Builds an RPC receipt response w.r.t. data layout of network.

use reth_rpc_eth_api::{helpers::LoadReceipt, EthApiTypes};
use reth_rpc_eth_types::EthStateCache;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadReceipt for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: EthApiTypes,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
