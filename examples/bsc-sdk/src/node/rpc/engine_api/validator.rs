use crate::{chainspec::BscChainSpec, hardforks::BscHardforks};
use alloy_consensus::{BlockHeader, Header, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::{eip4895::Withdrawal, Decodable2718};
use alloy_primitives::{B256, U256};
use alloy_rpc_types_engine::{ExecutionPayloadV1, PayloadAttributes, PayloadError};
use bytes::BufMut;
use reth::{
    api::{FullNodeComponents, NodeTypes},
    builder::{rpc::EngineValidatorBuilder, AddOnsContext},
    consensus::ConsensusError,
};
use reth_engine_primitives::{EngineValidator, ExecutionPayload, PayloadValidator};
use reth_ethereum_primitives::Block;
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
    PayloadTypes,
};
use reth_primitives::{EthPrimitives, RecoveredBlock, SealedBlock};
use reth_primitives_traits::Block as _;
use reth_trie_common::HashedPostState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::payload::BscPayloadTypes;

#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for BscEngineValidatorBuilder
where
    Types:
        NodeTypes<ChainSpec = BscChainSpec, Payload = BscPayloadTypes, Primitives = EthPrimitives>,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = BscEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(BscEngineValidator::new(Arc::new(ctx.config.chain.clone().as_ref().clone().into())))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BscExecutionData(pub Block);

impl ExecutionPayload for BscExecutionData {
    fn parent_hash(&self) -> B256 {
        self.0.parent_hash()
    }

    fn block_hash(&self) -> B256 {
        self.0.header.hash_slow()
    }

    fn block_number(&self) -> u64 {
        self.0.number()
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        None
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        None
    }

    fn timestamp(&self) -> u64 {
        self.0.timestamp()
    }

    fn gas_used(&self) -> u64 {
        self.0.gas_used()
    }
}

impl PayloadValidator for BscEngineValidator {
    type Block = Block;
    type ExecutionData = BscExecutionData;

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
    Types: PayloadTypes<PayloadAttributes = PayloadAttributes, ExecutionData = BscExecutionData>,
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
        payload: BscExecutionData,
    ) -> Result<SealedBlock<Block>, PayloadError> {
        let block = payload.0;

        let expected_hash = block.header.hash_slow();

        // First parse the block
        let sealed_block = block.seal_slow();

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
