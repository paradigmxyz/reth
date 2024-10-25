use alloy_rpc_types_beacon::relay::{
    BuilderBlockValidationRequest, BuilderBlockValidationRequestV2, BuilderBlockValidationRequestV3,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::ChainSpecProvider;
use reth_provider::{
    AccountReader, BlockReaderIdExt, HeaderProvider, StateProviderFactory, WithdrawalsProvider,
};
use reth_rpc_api::BlockSubmissionValidationApiServer;
use reth_rpc_server_types::result::internal_rpc_err;
use std::sync::Arc;
use tracing::warn;

/// The type that implements the `validation` rpc namespace trait
pub struct ValidationApi<Provider> {
    inner: Arc<ValidationApiInner<Provider>>,
}

impl<Provider> ValidationApi<Provider>
where
    Provider: BlockReaderIdExt
        + ChainSpecProvider
        + StateProviderFactory
        + HeaderProvider
        + AccountReader
        + WithdrawalsProvider
        + Clone
        + 'static,
{
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> Provider {
        self.inner.provider.clone()
    }

    /// Create a new instance of the [`ValidationApi`]
    pub fn new(provider: Provider) -> Self {
        let inner = Arc::new(ValidationApiInner { provider });
        Self { inner }
    }
}

#[async_trait]
impl<Provider> BlockSubmissionValidationApiServer for ValidationApi<Provider>
where
    Provider: BlockReaderIdExt
        + ChainSpecProvider
        + StateProviderFactory
        + HeaderProvider
        + AccountReader
        + WithdrawalsProvider
        + Clone
        + 'static,
{
    async fn validate_builder_submission_v1(
        &self,
        _request: BuilderBlockValidationRequest,
    ) -> RpcResult<()> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn validate_builder_submission_v2(
        &self,
        _request: BuilderBlockValidationRequestV2,
    ) -> RpcResult<()> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v3(
        &self,
        request: BuilderBlockValidationRequestV3,
    ) -> RpcResult<()> {
        warn!("flashbots_validateBuilderSubmissionV3: blindly accepting request without validation {:?}", request);
        Ok(())
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
