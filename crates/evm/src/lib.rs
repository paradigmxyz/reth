//! Traits for configuring an EVM specifics.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_primitives::{revm::env::fill_block_env, Address, ChainSpec, Header, Transaction, U256};
use revm::{Database, Evm, EvmBuilder};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, SpecId, TxEnv};

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

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<'a, DB: Database + 'a>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> Evm<'a, (), DB> {
        let mut evm = self.evm(db);
        evm.modify_spec_id(env.spec_id());
        evm.context.evm.env = env.env;
        evm
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env_and_inspector<'a, DB: Database + 'a, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Evm<'a, I, DB> {
        let mut evm = self.evm_with_inspector(db, inspector);
        evm.modify_spec_id(env.spec_id());
        evm.context.evm.env = env.env;
        evm
    }

    /// Returns a new EVM with the given inspector.
    ///
    /// Caution: This does not automatically configure the EVM with [ConfigureEvmEnv] methods. It is
    /// up to the caller to call an appropriate method to fill the transaction and block
    /// environment before executing any transactions using the provided EVM.
    fn evm_with_inspector<'a, DB: Database + 'a, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB> {
        EvmBuilder::default().with_db(db).with_external_context(inspector).build()
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
