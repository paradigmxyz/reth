//! Estimate gas needed implementation

use super::{LoadPendingBlock, LoadState};
use crate::helpers::{blocking_task, call};
use alloy_primitives::U256;
use alloy_rpc_types::{state::StateOverride, BlockId};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use futures::Future;
use std::sync::Arc;

/// Gas estimator implementation
#[derive(Debug)]
pub struct GasEstimator<T> {
    inner: Arc<T>,
}

impl<T> GasEstimator<T>
where
    T: LoadPendingBlock
        + LoadState
        + blocking_task::SpawnBlocking
        + call::Call
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Creates a new gas estimator instance
    pub const fn new(inner: Arc<T>) -> Self {
        Self { inner }
    }

    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    pub async fn estimate_gas_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<U256, T::Error>> + Send {
        let inner = self.inner.clone();

        async move {
            let (cfg, block_env, at) = inner.evm_env_at(at).await?;

            inner
                .spawn_blocking_io(move |this| {
                    let state = this.state_at_block_id(at)?;
                    this.estimate_gas_with(cfg, block_env, request, state, state_override)
                })
                .await
        }
    }
}
