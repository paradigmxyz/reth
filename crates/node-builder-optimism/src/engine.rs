use reth_node_api::{
    optimism_validate_version_specific_fields, AttributesValidationError, EngineApiMessageVersion,
    EngineTypes, EvmEnvConfig, PayloadOrAttributes,
};
use reth_payload_builder::{EthBuiltPayload, OptimismPayloadBuilderAttributes};
use reth_primitives::{
    revm::{config::revm_spec, env::fill_op_tx_env},
    revm_primitives::{AnalysisKind, CfgEnv, TxEnv},
    Address, Bytes, ChainSpec, Head, Header, Transaction, U256,
};
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
