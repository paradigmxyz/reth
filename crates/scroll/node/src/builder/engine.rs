use alloy_primitives::U256;
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
use reth_node_api::{
    MessageValidationKind, NewPayloadError, PayloadValidator, VersionSpecificValidationError,
};
use reth_node_builder::{
    rpc::EngineValidatorBuilder, AddOnsContext, EngineApiMessageVersion,
    EngineObjectValidationError, EngineTypes, EngineValidator, ExecutionPayload,
    FullNodeComponents, PayloadOrAttributes,
};
use reth_node_types::NodeTypes;
use reth_primitives_traits::{Block as _, RecoveredBlock};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::{try_into_block, ScrollEngineTypes};
use reth_scroll_primitives::{ScrollBlock, ScrollPrimitives};
use scroll_alloy_hardforks::ScrollHardforks;
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
        payload_or_attrs: PayloadOrAttributes<'_, Self::ExecutionData, ScrollPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_scroll_payload_or_attributes(
            &payload_or_attrs,
            payload_or_attrs.message_validation_kind(),
        )?;
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        attributes: &ScrollPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_scroll_payload_or_attributes(
            &PayloadOrAttributes::PayloadAttributes::<'_, ExecutionData, _>(attributes),
            MessageValidationKind::PayloadAttributes,
        )?;

        // ensure block data hint is present pre euclid.
        let is_euclid_active =
            self.chainspec.is_euclid_active_at_timestamp(attributes.payload_attributes.timestamp);
        if !is_euclid_active && attributes.block_data_hint.is_none() {
            return Err(EngineObjectValidationError::InvalidParams(
                "Missing block data hint Pre-Euclid".to_string().into(),
            ));
        }

        Ok(())
    }
}

/// Validates the payload or attributes for Scroll.
fn validate_scroll_payload_or_attributes<Payload: ExecutionPayload>(
    payload_or_attributes: &PayloadOrAttributes<'_, Payload, ScrollPayloadAttributes>,
    message_validation_kind: MessageValidationKind,
) -> Result<(), EngineObjectValidationError> {
    if payload_or_attributes.parent_beacon_block_root().is_some() {
        return Err(message_validation_kind
            .to_error(VersionSpecificValidationError::ParentBeaconBlockRootNotSupportedBeforeV3));
    }
    if payload_or_attributes.withdrawals().is_some() {
        return Err(message_validation_kind
            .to_error(VersionSpecificValidationError::HasWithdrawalsPreShanghai));
    }

    Ok(())
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

        // Seal the block with the no-turn difficulty and return if hashes match.
        // We guess the difficulty, which should always be 1 or 2 on Scroll.
        // CLIQUE_NO_TURN_DIFFICULTY is used starting at Euclid, so we test this value first.
        block.header.difficulty = CLIQUE_NO_TURN_DIFFICULTY;
        let block_hash_no_turn = block.hash_slow();
        if block_hash_no_turn == expected_hash {
            return block
                .seal_unchecked(block_hash_no_turn)
                .try_recover()
                .map_err(|err| NewPayloadError::Other(err.into()));
        }

        // Seal the block with the in-turn difficulty and return if hashes match
        block.header.difficulty = CLIQUE_IN_TURN_DIFFICULTY;
        let block_hash_in_turn = block.hash_slow();
        if block_hash_in_turn == expected_hash {
            return block
                .seal_unchecked(block_hash_in_turn)
                .try_recover()
                .map_err(|err| NewPayloadError::Other(err.into()));
        }

        Err(PayloadError::BlockHash { execution: block_hash_no_turn, consensus: expected_hash }
            .into())
    }
}
