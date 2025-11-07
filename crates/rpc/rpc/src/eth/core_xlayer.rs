//! XLayer-specific extensions for EthApi

use super::core::{EthApi, EthApiInner};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::RpcNodeCore;
use reth_rpc_eth_types::LegacyRpcClient;
use std::sync::Arc;

/// XLayer: Extension methods for EthApiInner
impl<N, Rpc> EthApiInner<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert,
{
    /// Returns the legacy RPC client if configured.
    #[inline]
    pub fn legacy_rpc_client(&self) -> Option<&Arc<LegacyRpcClient>> {
        self.legacy_rpc_client.as_ref()
    }
}

/// XLayer: Implement LegacyRpc trait for EthApi to enable legacy RPC routing
impl<N, Rpc> reth_rpc_eth_api::helpers::LegacyRpc for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert,
{
    fn legacy_rpc_client(&self) -> Option<&Arc<LegacyRpcClient>> {
        self.inner.legacy_rpc_client()
    }
}

