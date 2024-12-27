use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, PayloadError};
use reth_ethereum_engine_primitives::{EthEngineTypes, EthPayloadAttributes};
use reth_node_api::PayloadValidator;
use reth_node_builder::{
    rpc::EngineValidatorBuilder, AddOnsContext, EngineApiMessageVersion,
    EngineObjectValidationError, EngineTypes, EngineValidator, FullNodeComponents,
    PayloadOrAttributes,
};
use reth_node_types::NodeTypesWithEngine;
use reth_primitives::{Block, EthPrimitives, SealedBlock, SealedBlockFor};
use reth_scroll_chainspec::ScrollChainSpec;

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
    NoopEngineValidator: EngineValidator<Types::Engine>,
{
    type Validator = NoopEngineValidator;

    async fn build(self, _ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(NoopEngineValidator)
    }
}

/// Noop engine validator used as default for Scroll.
#[derive(Debug, Clone)]
pub struct NoopEngineValidator;

impl<Types> EngineValidator<Types> for NoopEngineValidator
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

impl PayloadValidator for NoopEngineValidator {
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        _payload: ExecutionPayload,
        _sidecar: ExecutionPayloadSidecar,
    ) -> Result<SealedBlockFor<Self::Block>, PayloadError> {
        Ok(SealedBlock::default())
    }
}
