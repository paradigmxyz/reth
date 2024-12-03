//! Support for optimism specific witness RPCs.

use alloy_consensus::Header;
use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use jsonrpsee_core::RpcResult;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_payload_builder::OpPayloadBuilder;
use reth_primitives::SealedHeader;
use reth_provider::{BlockReaderIdExt, ProviderError, ProviderResult, StateProviderFactory};
pub use reth_rpc_api::DebugExecutionWitnessApiServer;
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use std::{fmt::Debug, sync::Arc};

/// An extension to the `debug_` namespace of the RPC API.
pub struct OpDebugWitnessApi<Provider, EvmConfig> {
    inner: Arc<OpDebugWitnessApiInner<Provider, EvmConfig>>,
}

impl<Provider, EvmConfig> OpDebugWitnessApi<Provider, EvmConfig> {
    /// Creates a new instance of the `OpDebugWitnessApi`.
    pub fn new(provider: Provider, evm_config: EvmConfig) -> Self {
        let builder = OpPayloadBuilder::new(evm_config);
        let inner = OpDebugWitnessApiInner { provider, builder };
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

impl<Provider, EvmConfig> DebugExecutionWitnessApiServer<OpPayloadAttributes>
    for OpDebugWitnessApi<Provider, EvmConfig>
where
    Provider: BlockReaderIdExt<Header = reth_primitives::Header>
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec = OpChainSpec>
        + 'static,
    EvmConfig: ConfigureEvm<Header = Header> + 'static,
{
    fn execute_payload(
        &self,
        parent_block_hash: B256,
        attributes: OpPayloadAttributes,
    ) -> RpcResult<ExecutionWitness> {
        let parent_header = self.parent_header(parent_block_hash).to_rpc_result()?;
        self.inner
            .builder
            .payload_witness(&self.inner.provider, parent_header, attributes)
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
}
