//! Helper trait for interfacing with [`FullNodeComponents`].

use reth_node_api::FullNodeComponents;
use reth_provider::{BlockReader, ProviderBlock, ProviderReceipt};
use reth_rpc_eth_types::EthStateCache;

/// Additional components, asides the core node components, needed to run `eth_` namespace API
/// server.
pub trait RpcNodeCoreExt: FullNodeComponents<Provider: BlockReader> {
    /// Returns handle to RPC cache service.
    fn cache(
        &self,
    ) -> &EthStateCache<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>;
}
