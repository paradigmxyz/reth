#![cfg(feature = "optimism")]
use reth_node_api::{
    optimism_validate_version_specific_fields, AttributesValidationError, EngineApiMessageVersion,
    EngineTypes, EvmEnvConfig, PayloadOrAttributes,
};
use reth_payload_builder::{EthBuiltPayload, OptimismPayloadBuilderAttributes};
use reth_primitives::{
    revm::{config::revm_spec, env::fill_op_tx_env},
    revm_primitives::{AnalysisKind, CfgEnvWithSpecId, TxEnv},
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

/// Optimism-related EVM configuration.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OptimismEvmConfig;

impl EvmEnvConfig for OptimismEvmConfig {
    type TxMeta = Bytes;

    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Bytes)
    where
        T: AsRef<Transaction>,
    {
        fill_op_tx_env(tx_env, transaction, sender, meta);
    }

    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithSpecId,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = revm_spec(
            chain_spec,
            Head {
                number: header.number,
                timestamp: header.timestamp,
                difficulty: header.difficulty,
                total_difficulty,
                hash: Default::default(),
            },
        );

        cfg_env.chain_id = chain_spec.chain().id();
        cfg_env.spec_id = spec_id;
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

        // optimism-specific configuration
        cfg_env.optimism = chain_spec.is_optimism();
    }
}
