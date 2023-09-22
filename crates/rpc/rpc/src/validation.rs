use crate::result::ToRpcResultExt;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_consensus_common::validation::full_validation;
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, StateProviderFactory, AccountReader, HeaderProvider, WithdrawalsProvider};
use reth_rpc_api::ValidationApiServer;
use reth_rpc_types::{
    ExecutionPayloadValidation,
    Message
};
use reth_rpc_types_compat::engine::payload::try_into_sealed_block;

use std::sync::Arc;

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
    pub fn new(provider: Provider) -> Self {
        let inner = Arc::new(ValidationApiInner { provider});
        Self { inner }
    }
}

#[async_trait]
impl<Provider> ValidationApiServer for ValidationApi<Provider>
where
    Provider: BlockReaderIdExt + ChainSpecProvider + ChangeSetReader + StateProviderFactory + HeaderProvider + AccountReader + WithdrawalsProvider + 'static,
{
    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v1(&self, message: Message, execution_payload: ExecutionPayloadValidation, signature: String) -> RpcResult<()>  {
        let block = try_into_sealed_block(execution_payload.into(), None).map_ok_or_rpc_err()?;
        let chain_spec =  self.provider().chain_spec();
        full_validation(&block, self.provider(), &chain_spec).map_ok_or_rpc_err()
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
}
