use crate::{chainspec::BscChainSpec, hardforks::BscHardforks};
use alloy_rpc_types_engine::{ExecutionData, PayloadAttributes, PayloadError};
use reth::{
    api::{FullNodeComponents, NodeTypes},
    builder::{rpc::EngineValidatorBuilder, AddOnsContext},
    consensus::ConsensusError,
};
use reth_engine_primitives::{EngineValidator, PayloadValidator};
use reth_ethereum_primitives::Block;
use reth_node_ethereum::{EthEngineTypes, EthereumEngineValidator};
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
    PayloadTypes,
};
use reth_primitives::{EthPrimitives, RecoveredBlock, SealedBlock};
use reth_primitives_traits::{Block as _, SignedTransaction};
use reth_trie_common::HashedPostState;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for BscEngineValidatorBuilder
where
    Types:
        NodeTypes<ChainSpec = BscChainSpec, Payload = EthEngineTypes, Primitives = EthPrimitives>,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = EthereumEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(EthereumEngineValidator::new(Arc::new(ctx.config.chain.clone().as_ref().clone().into())))
    }
}

/// Validator for Optimism engine API.
#[derive(Debug, Clone)]
pub struct BscEngineValidator {
    inner: BscExecutionPayloadValidator<BscChainSpec>,
}

impl BscEngineValidator {
    /// Instantiates a new validator.
    pub fn new(chain_spec: Arc<BscChainSpec>) -> Self {
        Self { inner: BscExecutionPayloadValidator { inner: chain_spec } }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &BscChainSpec {
        self.inner.chain_spec()
    }
}

impl PayloadValidator for BscEngineValidator {
    type Block = Block;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: Self::ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block =
            self.inner.ensure_well_formed_payload(payload).map_err(NewPayloadError::other)?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }

    fn validate_block_post_execution_with_hashed_state(
        &self,
        _state_updates: &HashedPostState,
        _block: &RecoveredBlock<Self::Block>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<Types> EngineValidator<Types> for BscEngineValidator
where
    Types: PayloadTypes<PayloadAttributes = PayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        _payload_or_attrs: PayloadOrAttributes<'_, Self::ExecutionData, PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }
}

/// Execution payload validator.
#[derive(Clone, Debug)]
pub struct BscExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    inner: Arc<ChainSpec>,
}

impl<ChainSpec> BscExecutionPayloadValidator<ChainSpec>
where
    ChainSpec: BscHardforks,
{
    /// Returns reference to chain spec.
    pub fn chain_spec(&self) -> &ChainSpec {
        &self.inner
    }

    pub fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, PayloadError> {
        let ExecutionData { payload, sidecar } = payload;

        let expected_hash = payload.block_hash();

        // First parse the block
        let sealed_block = payload.try_into_block_with_sidecar(&sidecar)?.seal_slow();

        // Ensure the hash included in the payload matches the block hash
        if expected_hash != sealed_block.hash() {
            return Err(PayloadError::BlockHash {
                execution: sealed_block.hash(),
                consensus: expected_hash,
            })?
        }

        Ok(sealed_block)
    }
}
