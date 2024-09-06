//! Traits for configuring an EVM specifics.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

use core::ops::Deref;

use crate::builder::RethEvmBuilder;
use reth_primitives::{Address, Header, TransactionSigned, TransactionSignedEcRecovered, U256};
use revm::{Database, Evm, GetInspector};
use revm_primitives::{
    BlockEnv, Bytes, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg, SpecId, TxEnv,
};

pub mod builder;
pub mod either;
pub mod execute;
#[cfg(feature = "std")]
pub mod metrics;
pub mod noop;
pub mod provider;
pub mod system_calls;

#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// Trait for configuring the EVM for executing full blocks.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// Associated type for the default external context that should be configured for the EVM.
    type DefaultExternalContext<'a>;

    /// Returns new EVM with the given database
    ///
    /// This does not automatically configure the EVM with [`ConfigureEvmEnv`] methods. It is up to
    /// the caller to call an appropriate method to fill the transaction and block environment
    /// before executing any transactions using the provided EVM.
    fn evm<DB: Database>(&self, db: DB) -> Evm<'_, Self::DefaultExternalContext<'_>, DB> {
        RethEvmBuilder::new(db, self.default_external_context()).build()
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<DB: Database>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> Evm<'_, Self::DefaultExternalContext<'_>, DB> {
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
    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Evm<'_, I, DB>
    where
        DB: Database,
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
    fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        RethEvmBuilder::new(db, self.default_external_context()).build_with_inspector(inspector)
    }

    /// Provides the default external context.
    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a>;
}

/// This represents the set of methods used to configure the EVM's environment before block
/// execution.
///
/// Default trait method  implementation is done w.r.t. L1.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone + 'static {
    /// Returns a [`TxEnv`] from a [`TransactionSignedEcRecovered`].
    fn tx_env(&self, transaction: &TransactionSignedEcRecovered) -> TxEnv {
        let mut tx_env = TxEnv::default();
        self.fill_tx_env(&mut tx_env, transaction.deref(), transaction.signer());
        tx_env
    }

    /// Fill transaction environment from a [`TransactionSigned`] and the given sender address.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address);

    /// Fill transaction environment with a system contract call.
    fn fill_tx_env_system_contract_call(
        &self,
        env: &mut Env,
        caller: Address,
        contract: Address,
        data: Bytes,
    );

    /// Fill [`CfgEnvWithHandlerCfg`] fields according to the chain spec and given header
    fn fill_cfg_env(
        &self,
        cfg_env: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        total_difficulty: U256,
    );

    /// Fill [`BlockEnv`] field according to the chain spec and given header
    fn fill_block_env(&self, block_env: &mut BlockEnv, header: &Header, after_merge: bool) {
        block_env.number = U256::from(header.number);
        block_env.coinbase = header.beneficiary;
        block_env.timestamp = U256::from(header.timestamp);
        if after_merge {
            block_env.prevrandao = Some(header.mix_hash);
            block_env.difficulty = U256::ZERO;
        } else {
            block_env.difficulty = header.difficulty;
            block_env.prevrandao = None;
        }
        block_env.basefee = U256::from(header.base_fee_per_gas.unwrap_or_default());
        block_env.gas_limit = U256::from(header.gas_limit);

        // EIP-4844 excess blob gas of this block, introduced in Cancun
        if let Some(excess_blob_gas) = header.excess_blob_gas {
            block_env.set_blob_excess_gas_and_price(excess_blob_gas);
        }
    }

    /// Convenience function to call both [`fill_cfg_env`](ConfigureEvmEnv::fill_cfg_env) and
    /// [`ConfigureEvmEnv::fill_block_env`].
    fn fill_cfg_and_block_env(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        total_difficulty: U256,
    ) {
        self.fill_cfg_env(cfg, header, total_difficulty);
        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        self.fill_block_env(block_env, header, after_merge);
    }
}
