//! `Eth` Sim bundle implementation and helpers.

use std::sync::Arc;

use alloy_rpc_types_mev::{SendBundleRequest, SimBundleOverrides, SimBundleResponse};
use jsonrpsee::core::RpcResult;
use reth_rpc_api::MevSimApiServer;
use reth_rpc_eth_api::helpers::{Call, EthTransactions, LoadPendingBlock};
use reth_rpc_eth_types::EthApiError;
use reth_tasks::pool::BlockingTaskGuard;
use tracing::info;

/// `Eth` sim bundle implementation.
pub struct EthSimBundle<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthSimBundleInner<Eth>>,
}

impl<Eth> EthSimBundle<Eth> {
    /// Create a new `EthSimBundle` instance.
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        Self { inner: Arc::new(EthSimBundleInner { eth_api, blocking_task_guard }) }
    }
}

impl<Eth> EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    /// Simulates a bundle of transactions.
    pub async fn sim_bundle(
        &self,
        request: SendBundleRequest,
        overrides: SimBundleOverrides,
    ) -> RpcResult<SimBundleResponse> {
        info!("mev_simBundle called, request: {:?}, overrides: {:?}", request, overrides);
        Err(EthApiError::Unsupported("mev_simBundle is not supported").into())
    }
}

#[async_trait::async_trait]
impl<Eth> MevSimApiServer for EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    async fn sim_bundle(
        &self,
        request: SendBundleRequest,
        overrides: SimBundleOverrides,
    ) -> RpcResult<SimBundleResponse> {
        Self::sim_bundle(self, request, overrides).await
    }
}

/// Container type for `EthSimBundle` internals
#[derive(Debug)]
struct EthSimBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    #[allow(dead_code)]
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    #[allow(dead_code)]
    blocking_task_guard: BlockingTaskGuard,
}

impl<Eth> std::fmt::Debug for EthSimBundle<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthSimBundle").finish_non_exhaustive()
    }
}

impl<Eth> Clone for EthSimBundle<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}
