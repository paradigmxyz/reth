#![cfg(feature = "optimism")]
use reth_node_api::{
    optimism_validate_version_specific_fields, AttributesValidationError, EngineApiMessageVersion,
    EngineTypes, PayloadOrAttributes,
};
use reth_payload_builder::{EthBuiltPayload, OptimismPayloadBuilderAttributes};
use reth_primitives::ChainSpec;
use reth_rpc_types::engine::OptimismPayloadAttributes;

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismEngineTypes;

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

/// Optimism-related EVM configuration.
#[derive(Debug)]
pub struct OptimismEvmConfig;

// TODO:
// * split up fill_tx_env into fill_tx_env and fill_tx_env_optimism
// * code duplication for op
// * make fill_tx_env_optimism accept regular fill_tx_env args
// * do the following code inside trait impl

// #[cfg(feature = "optimism")]
// {
//     let mut envelope_buf = Vec::with_capacity(transaction.length_without_header());
//     transaction.encode_enveloped(&mut envelope_buf);
//     fill_tx_env(&mut self.evm.env.tx, transaction, sender, envelope_buf.into());
// }
