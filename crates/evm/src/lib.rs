//! Traits for configuring an EVM specifics.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use execute::EvmExecutor;
use reth_primitives::{
    revm::env::fill_block_env, Address, ChainSpec, Header, TransactionSigned, U256,
};
use revm::{inspector_handle_register, Database, DatabaseCommit, Evm, EvmBuilder, GetInspector};
use revm_primitives::{
    BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg, SpecId, TxEnv,
};

pub mod either;
pub mod execute;
pub mod noop;
pub mod provider;

#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// Trait for configuring the EVM for executing full blocks.
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// Associated type for the default external context that should be configured for the EVM.
    type DefaultExternalContext<'a>;

    /// Returns new EVM with the given database
    ///
    /// This does not automatically configure the EVM with [`ConfigureEvmEnv`] methods. It is up to
    /// the caller to call an appropriate method to fill the transaction and block environment
    /// before executing any transactions using the provided EVM.
    fn evm<'a, DB: Database + 'a>(
        &'a self,
        db: DB,
    ) -> Evm<'a, Self::DefaultExternalContext<'a>, DB>;

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<'a, DB: Database + 'a>(
        &'a self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        let mut evm = self.evm(db);
        evm.modify_spec_id(env.spec_id());
        evm.context.evm.env = env.env;
        evm
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will use the given external inspector as the EVM external context.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env_and_inspector<'a, DB, I>(
        &'a self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Evm<'a, I, DB>
    where
        DB: Database + 'a,
        I: GetInspector<DB>,
    {
        let mut evm = self.evm_with_inspector(db, inspector);
        evm.modify_spec_id(env.spec_id());
        evm.context.evm.env = env.env;
        evm
    }

    /// Returns a new EVM with the given inspector.
    ///
    /// Caution: This does not automatically configure the EVM with [`ConfigureEvmEnv`] methods. It
    /// is up to the caller to call an appropriate method to fill the transaction and block
    /// environment before executing any transactions using the provided EVM.
    fn evm_with_inspector<'a, DB, I>(&'a self, db: DB, inspector: I) -> Evm<'a, I, DB>
    where
        DB: Database + 'a,
        I: GetInspector<DB>,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .append_handler_register(inspector_handle_register)
            .build()
    }
}

/// This represents the set of methods used to configure the EVM's environment before block
/// execution.
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone + 'static {
    /// Fill transaction environment from a [`TransactionSigned`] and the given sender address.
    fn fill_tx_env(tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address);

    /// Fill [`CfgEnvWithHandlerCfg`] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [`fill_cfg_env`](ConfigureEvmEnv::fill_cfg_env) and
    /// [`fill_block_env`].
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

/// Trait for managing the EVM context.
pub trait EvmContext<EXT, DB: Database> {
    /// The executor produced with the context set.
    type Executor: EvmExecutor<DB>;

    /// Sets the EVM database.
    #[must_use]
    fn with_db(self, db: DB) -> Self;

    /// Sets the EVM environment.
    #[must_use]
    fn with_env(self, env: Env) -> Self;

    /// Sets the EVM configuration environment.
    #[must_use]
    fn with_cfg_env(self, cfg: CfgEnv) -> Self;

    /// Sets the block environment.
    #[must_use]
    fn with_block_env(self, block: BlockEnv) -> Self;

    /// Sets the transaction environment.
    #[must_use]
    fn with_tx_env(self, tx: TxEnv) -> Self;

    /// Sets the specification (hardfork) for the EVM instance.
    #[must_use]
    fn with_spec_id(self, spec_id: SpecId) -> Self;

    /// Sets the inspector for the EVM instance.
    #[must_use]
    fn with_inspector(self, inspector: EXT) -> Self;

    /// Returns the EVM executor.
    fn executor(self) -> Self::Executor;
}

/// A context for configuring a Revm EVM instance.
#[allow(missing_debug_implementations)]
pub struct RevmContext<'a, Stage, EXT, DB: Database>(EvmBuilder<'a, Stage, EXT, DB>);

impl<'a, Stage, EXT, DB> EvmContext<EXT, DB> for RevmContext<'a, Stage, EXT, DB>
where
    DB: Database + DatabaseCommit + 'a,
{
    type Executor = Evm<'a, EXT, DB>;

    fn with_db(self, db: DB) -> Self {
        Self(self.0.modify_db(|old| *old = db))
    }

    fn with_env(self, env: Env) -> Self {
        Self(self.0.modify_env(|old| *old = env.into()))
    }

    fn with_cfg_env(self, cfg: CfgEnv) -> Self {
        Self(self.0.modify_cfg_env(|old| *old = cfg))
    }

    fn with_block_env(self, block: BlockEnv) -> Self {
        Self(self.0.modify_block_env(|old| *old = block))
    }

    fn with_tx_env(self, tx: TxEnv) -> Self {
        Self(self.0.modify_tx_env(|old| *old = tx))
    }

    fn with_spec_id(self, spec_id: SpecId) -> Self {
        Self(self.0.with_spec_id(spec_id))
    }

    fn with_inspector(self, inspector: EXT) -> Self {
        Self(self.0.modify_external_context(|old| *old = inspector))
    }

    fn executor(self) -> Self::Executor {
        self.0.build()
    }
}
