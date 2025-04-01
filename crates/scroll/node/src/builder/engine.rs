use alloy_primitives::U256;
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
use reth_node_api::{NewPayloadError, PayloadValidator};
use reth_node_builder::{
    rpc::EngineValidatorBuilder, AddOnsContext, EngineApiMessageVersion,
    EngineObjectValidationError, EngineTypes, EngineValidator, FullNodeComponents,
    PayloadOrAttributes,
};
use reth_node_types::NodeTypes;
use reth_primitives_traits::{Block as _, RecoveredBlock};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::{try_into_block, ScrollEngineTypes};
use reth_scroll_primitives::{ScrollBlock, ScrollPrimitives};
use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;
use std::sync::Arc;

/// The block difficulty for in turn signing in the Clique consensus.
const CLIQUE_IN_TURN_DIFFICULTY: U256 = U256::from_limbs([2, 0, 0, 0]);
/// The block difficulty for out of turn signing in the Clique consensus.
const CLIQUE_NO_TURN_DIFFICULTY: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Builder for [`ScrollEngineValidator`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for ScrollEngineValidatorBuilder
where
    Types: NodeTypes<
        ChainSpec = ScrollChainSpec,
        Primitives = ScrollPrimitives,
        Payload = ScrollEngineTypes,
    >,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = ScrollEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        let chainspec = ctx.config.chain.clone();
        Ok(ScrollEngineValidator { chainspec })
    }
}

/// Scroll engine validator.
#[derive(Debug, Clone)]
pub struct ScrollEngineValidator {
    chainspec: Arc<ScrollChainSpec>,
}

impl ScrollEngineValidator {
    /// Returns a new [`ScrollEngineValidator`].
    pub const fn new(chainspec: Arc<ScrollChainSpec>) -> Self {
        Self { chainspec }
    }
}

impl<Types> EngineValidator<Types> for ScrollEngineValidator
where
    Types: EngineTypes<PayloadAttributes = ScrollPayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        _payload_or_attrs: PayloadOrAttributes<'_, Self::ExecutionData, ScrollPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &ScrollPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }
}

impl PayloadValidator for ScrollEngineValidator {
    type Block = ScrollBlock;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let expected_hash = payload.payload.block_hash();

        // First parse the block
        let mut block = try_into_block(payload, self.chainspec.clone())?;

        // Seal the block with the in-turn difficulty and return if hashes match
        block.header.difficulty = CLIQUE_IN_TURN_DIFFICULTY;
        let block_hash_in_turn = block.hash_slow();
        if block_hash_in_turn == expected_hash {
            return block
                .seal_unchecked(block_hash_in_turn)
                .try_recover()
                .map_err(|err| NewPayloadError::Other(err.into()));
        }

        // Seal the block with the no-turn difficulty and return if hashes match
        block.header.difficulty = CLIQUE_NO_TURN_DIFFICULTY;
        let block_hash_no_turn = block.hash_slow();
        if block_hash_no_turn == expected_hash {
            return block
                .seal_unchecked(block_hash_no_turn)
                .try_recover()
                .map_err(|err| NewPayloadError::Other(err.into()));
        }

        Err(PayloadError::BlockHash { execution: block_hash_no_turn, consensus: expected_hash }
            .into())
    }
}
