use reth::revm::{Database, Evm, EvmBuilder, GetInspector};
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{
    revm_primitives::{CfgEnvWithHandlerCfg, TxEnv},
    Address, ChainSpec, Header, Transaction, U256,
};

/// Custom EVM configuration
#[derive(Debug, Clone, Copy, Default)]
pub struct Config;

impl ConfigureEvmEnv for Config {
    type TxMeta = ();

    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>,
    {
        EthEvmConfig::fill_tx_env(tx_env, transaction, sender, meta)
    }

    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        EthEvmConfig::fill_cfg_env(cfg_env, chain_spec, header, total_difficulty)
    }
}

impl ConfigureEvm for Config {
    fn evm<'a, DB: Database + 'a>(&self, db: DB) -> Evm<'a, (), DB> {
        EvmBuilder::default().with_db(db).build()
    }

    fn evm_with_inspector<'a, DB, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB>
    where
        DB: Database + 'a,
        I: GetInspector<DB>,
    {
        EvmBuilder::default().with_db(db).with_external_context(inspector).build()
    }
}
