use crate::eth::error::{EthApiError, EthResult};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::Bytes;
use reth_provider::{BlockReaderIdExt, ChangeSetReader, StateProviderFactory};
use reth_rpc_api::ValidationApiServer;
use reth_rpc_types::{
    ExecutionPayload,
    Message
};
use reth_rpc_types_compat::engine::payload::try_into_sealed_block;

use reth_tasks::TaskSpawner;
use std::{ future::Future, sync::Arc};
use tokio::sync::oneshot;
use tracing::info;

/// `validation` API implementation.
///
/// This type provides the functionality for handling `validation` prototype RPC requests.
pub struct ValidationApi<Provider> {
    inner: Arc<ValidationApiInner<Provider>>,
}

// === impl ValidationApi ===

impl<Provider> ValidationApi<Provider> {
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Create a new instance of the [ValidationApi]
    pub fn new(provider: Provider, task_spawner: Box<dyn TaskSpawner>) -> Self {
        let inner = Arc::new(ValidationApiInner { provider, task_spawner });
        Self { inner }
    }
}

impl<Provider> ValidationApi<Provider>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
{
    /// Executes the future on a new blocking task.
    async fn on_blocking_task<C, F, R>(&self, c: C) -> EthResult<R>
    where
        C: FnOnce(Self) -> F,
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let f = c(this);
        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let res = f.await;
            let _ = tx.send(res);
        }));
        rx.await.map_err(|_| EthApiError::InternalEthError)?
    }
}

#[async_trait]
impl<Provider> ValidationApiServer for ValidationApi<Provider>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
{
    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v1(&self, message: Message, execution_payload: ExecutionPayload, signature: String) -> RpcResult<Bytes>  {
        let block = try_into_sealed_block(execution_payload, Some(message.parent_hash)).unwrap();
        info!("block: {:?}", block);
        let empty_bytes = Bytes::from([0u8; 0]);
        Ok(empty_bytes)
    }
}

impl<Provider> std::fmt::Debug for ValidationApi<Provider> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationApi").finish_non_exhaustive()
    }
}

impl<Provider> Clone for ValidationApi<Provider> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct ValidationApiInner<Provider> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
}
