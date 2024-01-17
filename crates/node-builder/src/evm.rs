use reth_node_api::EvmEnvConfig;
use reth_primitives::{
    revm::env::{fill_cfg_env, fill_tx_env},
    revm_primitives::{CfgEnv, TxEnv},
    Address, ChainSpec, Header, Transaction, U256,
};

/// Ethereum-related EVM configuration.
#[derive(Debug)]
pub struct EthEvmConfig;

impl EvmEnvConfig for EthEvmConfig {
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address)
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
        fill_cfg_env(cfg_env, chain_spec, header, total_difficulty)
    }
}
