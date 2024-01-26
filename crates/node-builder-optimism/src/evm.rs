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
        cfg_env: &mut CfgEnv,
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::revm_primitives::BlockEnv;

    #[test]
    #[ignore]
    fn test_fill_cfg_and_block_env() {
        let mut cfg_env = CfgEnv::default();
        let mut block_env = BlockEnv::default();
        let header = Header::default();
        let chain_spec = ChainSpec::default();
        let total_difficulty = U256::ZERO;

        EthEvmConfig::fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }
}
