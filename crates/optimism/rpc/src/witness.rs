//! Support for optimism specific witness RPCs.

use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use jsonrpsee_core::{async_trait, RpcResult};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_chainspec::ChainSpecProvider;
use reth_evm::{execute::BlockExecutionStrategyFactory, ConfigureEvm};
use reth_node_api::NodePrimitives;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_payload_builder::{OpPayloadBuilder, OpPayloadPrimitives};
use reth_primitives::SealedHeader;
use reth_provider::{
    BlockReaderIdExt, NodePrimitivesProvider, ProviderError, ProviderResult, StateProviderFactory,
};
pub use reth_rpc_api::DebugExecutionWitnessApiServer;
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{fmt::Debug, sync::Arc};
use tokio::sync::{oneshot, Semaphore};

/// An extension to the `debug_` namespace of the RPC API.
pub struct OpDebugWitnessApi<Pool, Provider: NodePrimitivesProvider, EvmConfig: ConfigureEvm> {
    inner: Arc<OpDebugWitnessApiInner<Pool, Provider, EvmConfig>>,
}

impl<Pool, Provider: NodePrimitivesProvider, EvmConfig: ConfigureEvm>
    OpDebugWitnessApi<Pool, Provider, EvmConfig>
{
    /// Creates a new instance of the `OpDebugWitnessApi`.
    pub fn new(
        provider: Provider,
        task_spawner: Box<dyn TaskSpawner>,
        builder: OpPayloadBuilder<Pool, Provider, EvmConfig, Provider::Primitives>,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(3));
        let inner = OpDebugWitnessApiInner { provider, builder, task_spawner, semaphore };
        Self { inner: Arc::new(inner) }
    }
}

impl<Pool, Provider, EvmConfig> OpDebugWitnessApi<Pool, Provider, EvmConfig>
where
    EvmConfig: ConfigureEvm,
    Provider: NodePrimitivesProvider + BlockReaderIdExt<Header = reth_primitives::Header>,
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
impl<Pool, Provider, EvmConfig> DebugExecutionWitnessApiServer<OpPayloadAttributes>
    for OpDebugWitnessApi<Pool, Provider, EvmConfig>
where
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = <Provider::Primitives as NodePrimitives>::SignedTx,
            >,
        > + 'static,
    Provider: BlockReaderIdExt<Header = reth_primitives::Header>
        + NodePrimitivesProvider<Primitives: OpPayloadPrimitives>
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec = OpChainSpec>
        + Clone
        + 'static,
    EvmConfig: BlockExecutionStrategyFactory<Primitives = Provider::Primitives> + 'static,
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
            let res = this.inner.builder.payload_witness(parent_header, attributes);
            let _ = tx.send(res);
        }));

        rx.await
            .map_err(|err| internal_rpc_err(err.to_string()))?
            .map_err(|err| internal_rpc_err(err.to_string()))
    }
}

impl<Pool, Provider, EvmConfig> Clone for OpDebugWitnessApi<Pool, Provider, EvmConfig>
where
    EvmConfig: ConfigureEvm,
    Provider: NodePrimitivesProvider,
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}
impl<Pool, Provider, EvmConfig> Debug for OpDebugWitnessApi<Pool, Provider, EvmConfig>
where
    EvmConfig: ConfigureEvm,
    Provider: NodePrimitivesProvider,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpDebugWitnessApi").finish_non_exhaustive()
    }
}

struct OpDebugWitnessApiInner<Pool, Provider: NodePrimitivesProvider, EvmConfig: ConfigureEvm> {
    provider: Provider,
    builder: OpPayloadBuilder<Pool, Provider, EvmConfig, Provider::Primitives>,
    task_spawner: Box<dyn TaskSpawner>,
    semaphore: Arc<Semaphore>,
}
