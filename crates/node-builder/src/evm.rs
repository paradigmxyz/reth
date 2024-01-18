use reth_node_api::EvmEnvConfig;
use reth_primitives::{
    revm::{config::revm_spec, env::fill_tx_env},
    revm_primitives::{AnalysisKind, CfgEnv, TxEnv},
    Address, ChainSpec, Head, Header, Transaction, U256,
};

/// Ethereum-related EVM configuration.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct EthEvmConfig;

impl EvmEnvConfig for EthEvmConfig {
    type TxMeta = ();

    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, _meta: ())
    where
        T: AsRef<Transaction>,
    {
        fill_tx_env(tx_env, transaction, sender)
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
    }
}
