use reth_node_api::EvmEnvConfig;
use reth_primitives::{
    revm::env::{fill_cfg_env, fill_tx_env},
    revm_primitives::{CfgEnv, TxEnv},
    Address, ChainSpec, Header, Transaction, U256,
};

/// Ethereum-related EVM configuration.
#[derive(Debug)]
pub struct EthEvmConfig;

// TODO: remove after implementing
// #[cfg(feature = "optimism")]
/// Optimism-related EVM configuration.
#[derive(Debug)]
pub struct OptimismEvmConfig;

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

// this is default eth fill_tx_env
// // Fill revm structure.
// #[cfg(not(feature = "optimism"))]
// fill_tx_env(&mut self.evm.env.tx, transaction, sender);
