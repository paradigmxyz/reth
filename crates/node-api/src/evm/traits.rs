use reth_primitives::{revm::env::fill_block_env, Address, ChainSpec, Header, Transaction, U256};
use revm::{interpreter::Host, Database, Evm, EvmBuilder, Handler};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, SpecId, TxEnv};

/// Trait for configuring the EVM for executing full blocks.
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// Returns new EVM with the given database
    ///
    /// This does not automatically configure the EVM with [ConfigureEvmEnv] methods. It is up to
    /// the caller to call an appropriate method to fill the transaction and block environment
    /// before executing any transactions using the provided EVM.
    fn evm<'a, DB: Database + 'a>(&self, db: DB) -> Evm<'a, (), DB> {
        EvmBuilder::default().with_db(db).build()
    }

    /// Returns a new EVM with the given inspector.
    ///
    /// This does not automatically configure the EVM with [ConfigureEvmEnv] methods. It is up to
    /// the caller to call an appropriate method to fill the transaction and block environment
    /// before executing any transactions using the provided EVM.
    fn evm_with_inspector<'a, DB: Database + 'a, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB> {
        EvmBuilder::default().with_db(db).with_external_context(inspector).build()
    }

    /// Returns a new EVM with the given handler and database.
    fn evm_with_handler<'a, DB: Database + 'a, I, H: Host>(
        &self,
        handler: Handler<'a, H, I, DB>,
        db: DB,
    ) -> H {
        EvmBuilder::default().with_db(db).with_handler(handler).build()
    }

    /// Returns a new EVM with the given handler, inspector, and database.
    fn evm_with_handler_and_inspector<'a, DB: Database + 'a, I, H: Host>(
        &self,
        handler: Handler<'a, H, I, DB>,
        inspector: I,
        db: DB,
    ) -> H {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .with_handler(handler)
            .build()
    }
}

/// This represents the set of methods used to configure the EVM's environment before block
/// execution.
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone {
    /// The type of the transaction metadata that should be used to fill fields in the transaction
    /// environment.
    ///
    /// On ethereum mainnet, this is `()`, and on optimism these are the L1 fee fields and
    /// additional L1 block info.
    type TxMeta;

    /// Fill transaction environment from a [Transaction] and the given sender address.
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>;

    /// Fill [CfgEnvWithHandlerCfg] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [fill_cfg_env](ConfigureEvmEnv::fill_cfg_env) and
    /// [fill_block_env].
    fn fill_cfg_and_block_env(
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        Self::fill_cfg_env(cfg, chain_spec, header, total_difficulty);
        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        fill_block_env(block_env, chain_spec, header, after_merge);
    }
}
