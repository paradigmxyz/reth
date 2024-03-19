use reth_node_api::{
    validate_version_specific_fields, EngineApiMessageVersion, EngineObjectValidationError,
    EngineTypes, PayloadOrAttributes,
};
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::ChainSpec;
use reth_rpc_types::{
    engine::{
        ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
        PayloadAttributes as EthPayloadAttributes,
    },
    ExecutionPayloadV1,
};

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;
    type ExecutionPayloadV1 = ExecutionPayloadV1;
    type ExecutionPayloadV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadV3 = ExecutionPayloadEnvelopeV3;

    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(chain_spec, version, payload_or_attrs)
    }
}
