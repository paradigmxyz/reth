//! Support for optimism specific witness RPCs.

use alloy_consensus::Header;
use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use jsonrpsee_core::{async_trait, RpcResult};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_payload_builder::OpPayloadBuilder;
use reth_primitives::{SealedHeader, TransactionSigned};
use reth_provider::{BlockReaderIdExt, ProviderError, ProviderResult, StateProviderFactory};
pub use reth_rpc_api::DebugExecutionWitnessApiServer;
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use reth_tasks::TaskSpawner;
use std::{fmt::Debug, sync::Arc};
use tokio::sync::{oneshot, Semaphore};

/// An extension to the `debug_` namespace of the RPC API.
pub struct OpDebugWitnessApi<Provider, EvmConfig> {
    inner: Arc<OpDebugWitnessApiInner<Provider, EvmConfig>>,
}

impl<Provider, EvmConfig> OpDebugWitnessApi<Provider, EvmConfig> {
    /// Creates a new instance of the `OpDebugWitnessApi`.
    pub fn new(
        provider: Provider,
        evm_config: EvmConfig,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let builder = OpPayloadBuilder::new(evm_config);
        let semaphore = Arc::new(Semaphore::new(3));
        let inner = OpDebugWitnessApiInner { provider, builder, task_spawner, semaphore };
        Self { inner: Arc::new(inner) }
    }
}

impl<Provider, EvmConfig> OpDebugWitnessApi<Provider, EvmConfig>
where
    Provider: BlockReaderIdExt<Header = reth_primitives::Header>,
{
    /// Fetches the parent header by hash.
    fn parent_header(&self, parent_block_hash: B256) -> ProviderResult<SealedHeader> {
        self.inner
            .provider
            .sealed_header_by_hash(parent_block_hash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(parent_block_hash.into()))
    }
}

#[async_trait]
impl<Provider, EvmConfig> DebugExecutionWitnessApiServer<OpPayloadAttributes>
    for OpDebugWitnessApi<Provider, EvmConfig>
where
    Provider: BlockReaderIdExt<Header = reth_primitives::Header>
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec = OpChainSpec>
        + Clone
        + 'static,
    EvmConfig: ConfigureEvm<Header = Header, Transaction = TransactionSigned> + 'static,
{
    async fn execute_payload(
        &self,
        parent_block_hash: B256,
        attributes: OpPayloadAttributes,
    ) -> RpcResult<ExecutionWitness> {
        let _permit = self.inner.semaphore.acquire().await;

        let parent_header = self.parent_header(parent_block_hash).to_rpc_result()?;

        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let res =
                this.inner.builder.payload_witness(&this.inner.provider, parent_header, attributes);
            let _ = tx.send(res);
        }));

        rx.await
            .map_err(|err| internal_rpc_err(err.to_string()))?
            .map_err(|err| internal_rpc_err(err.to_string()))
    }
}

impl<Provider, EvmConfig> Clone for OpDebugWitnessApi<Provider, EvmConfig> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}
impl<Provider, EvmConfig> Debug for OpDebugWitnessApi<Provider, EvmConfig> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpDebugWitnessApi").finish_non_exhaustive()
    }
}

struct OpDebugWitnessApiInner<Provider, EvmConfig> {
    provider: Provider,
    builder: OpPayloadBuilder<EvmConfig>,
    task_spawner: Box<dyn TaskSpawner>,
    semaphore: Arc<Semaphore>,
}
