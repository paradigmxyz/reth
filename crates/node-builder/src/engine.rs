#[cfg(feature = "optimism")]
use reth_node_api::optimism_validate_version_specific_fields;
use reth_node_api::{
    validate_version_specific_fields, AttributesValidationError, EngineApiMessageVersion,
    EngineTypes, PayloadOrAttributes,
};
#[cfg(feature = "optimism")]
use reth_payload_builder::OptimismPayloadBuilderAttributes;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::ChainSpec;
#[cfg(feature = "optimism")]
use reth_rpc_types::engine::OptimismPayloadAttributes;
use reth_rpc_types::engine::PayloadAttributes as EthPayloadAttributes;

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, EthPayloadAttributes>,
    ) -> Result<(), AttributesValidationError> {
        validate_version_specific_fields(chain_spec, version, payload_or_attrs)
    }
}

#[cfg(feature = "optimism")]
/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismEngineTypes;

// TODO: remove cfg once Hardfork::Canyon can be used without the flag
#[cfg(feature = "optimism")]
impl EngineTypes for OptimismEngineTypes {
    type PayloadAttributes = OptimismPayloadAttributes;
    type PayloadBuilderAttributes = OptimismPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, OptimismPayloadAttributes>,
    ) -> Result<(), AttributesValidationError> {
        optimism_validate_version_specific_fields(chain_spec, version, payload_or_attrs)
    }
}
