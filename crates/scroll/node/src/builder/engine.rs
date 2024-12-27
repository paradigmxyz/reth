use alloy_primitives::{Sealable, U256};
use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, PayloadError};
use reth_ethereum_engine_primitives::{EthEngineTypes, EthPayloadAttributes};
use reth_node_api::PayloadValidator;
use reth_node_builder::{
    rpc::EngineValidatorBuilder, AddOnsContext, EngineApiMessageVersion,
    EngineObjectValidationError, EngineTypes, EngineValidator, FullNodeComponents,
    PayloadOrAttributes,
};
use reth_node_types::NodeTypesWithEngine;
use reth_primitives::{Block, BlockExt, EthPrimitives, SealedBlockFor};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine::try_into_block;
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
    Types: NodeTypesWithEngine<
        ChainSpec = ScrollChainSpec,
        Primitives = EthPrimitives,
        Engine = EthEngineTypes,
    >,
    Node: FullNodeComponents<Types = Types>,
    ScrollEngineValidator: EngineValidator<Types::Engine>,
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

impl<Types> EngineValidator<Types> for ScrollEngineValidator
where
    Types: EngineTypes<PayloadAttributes = EthPayloadAttributes>,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        _payload_or_attrs: PayloadOrAttributes<'_, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &EthPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }
}

impl PayloadValidator for ScrollEngineValidator {
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionPayload,
        sidecar: ExecutionPayloadSidecar,
    ) -> Result<SealedBlockFor<Self::Block>, PayloadError> {
        let expected_hash = payload.block_hash();

        // First parse the block
        let mut block = try_into_block(payload, &sidecar, self.chainspec.clone())?;

        // Seal the block with the in-turn difficulty and return if hashes match
        block.header.difficulty = CLIQUE_IN_TURN_DIFFICULTY;
        let sealed_block_in_turn = block.seal_ref_slow();
        if sealed_block_in_turn.hash() == expected_hash {
            let hash = sealed_block_in_turn.hash();
            return Ok(block.seal(hash))
        }

        // Seal the block with the no-turn difficulty and return if hashes match
        block.header.difficulty = CLIQUE_NO_TURN_DIFFICULTY;
        let sealed_block_no_turn = block.seal_ref_slow();
        if sealed_block_no_turn.hash() == expected_hash {
            let hash = sealed_block_no_turn.hash();
            return Ok(block.seal(hash))
        }

        Err(PayloadError::BlockHash {
            execution: sealed_block_no_turn.hash(),
            consensus: expected_hash,
        })
    }
}
