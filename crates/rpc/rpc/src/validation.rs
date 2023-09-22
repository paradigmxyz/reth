use crate::eth::error::{EthApiError, EthResult};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_consensus_common::validation::full_validation;
use reth_primitives::Bytes;
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, StateProviderFactory, AccountReader, HeaderProvider, WithdrawalsProvider};
use reth_rpc_api::ValidationApiServer;
use reth_rpc_types::{
    ExecutionPayloadValidation,
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
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory  + ChainSpecProvider + 'static,
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
    Provider: BlockReaderIdExt + ChainSpecProvider + ChangeSetReader + StateProviderFactory + HeaderProvider + AccountReader + WithdrawalsProvider + 'static,
{
    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v1(&self, message: Message, execution_payload: ExecutionPayloadValidation, signature: String) -> RpcResult<Bytes>  {
        println!("execution_payload: {:#?}", execution_payload);
        let block = try_into_sealed_block(execution_payload.into(), None).unwrap();
        println!("BlockWithdrawals: {:#?}", block.withdrawals);
        info!(target: "reth::rpc::validation", "Block decoded");
        let chain_spec =  self.provider().chain_spec();
        full_validation(&block, self.provider(), &chain_spec).unwrap();
        Ok(block.header.hash().as_bytes().into())
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
