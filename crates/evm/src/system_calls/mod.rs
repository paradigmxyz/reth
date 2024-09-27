//! System contract call functions.

use crate::ConfigureEvm;
use core::fmt::Display;
use reth_chainspec::ChainSpec;
use reth_execution_errors::BlockExecutionError;
use reth_primitives::Header;
use revm::{Database, DatabaseCommit, Evm};
use revm_primitives::{BlockEnv, ResultAndState, B256};

mod eip2935;
pub use eip2935::*;

mod eip4788;
pub use eip4788::*;

mod eip7002;
pub use eip7002::*;

mod eip7251;
pub use eip7251::*;

/// A hook that is called after each state change.
// TODO impl for &mut
pub trait OnStateHook {
    /// Invoked with the result and state after each system call.
    fn on_state(&mut self, state: &ResultAndState);
}

impl<F> OnStateHook for F
where
    F: FnMut(&ResultAndState),
{
    fn on_state(&mut self, state: &ResultAndState) {
        self(state)
    }
}

/// An [`OnStateHook`] that does nothing.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct NoopHook;

impl OnStateHook for NoopHook {
    fn on_state(&mut self, _state: &ResultAndState) {}
}

/// An ephemeral helper type for executing system calls.
///
/// This can be used to chain system transaction calls.
#[allow(missing_debug_implementations)]
pub struct SystemCaller<'a, EvmConfig, Ext, DB: Database, Chainspec, Hook = NoopHook> {
    evm: Evm<'a, Ext, DB>,
    evm_config: EvmConfig,
    db: DB,
    chain_spec: Chainspec,
    /// Optional hook to be called after each state change.
    // TODO do we want this optional?
    hook: Option<Hook>,
}

impl<'a, EvmConfig, Ext, DB: Database, Chainspec> SystemCaller<'a, EvmConfig, Ext, DB, Chainspec> {
    /// Create a new system caller with the given EVM config, database, and chain spec.
    pub const fn new(
        evm_config: EvmConfig,
        db: DB,
        chain_spec: Chainspec,
        evm: Evm<'a, Ext, DB>,
    ) -> Self {
        Self { evm_config, db, chain_spec, evm, hook: None }
    }
}

impl<'a, EvmConfig, Ext, DB: Database, Chainspec, Hook>
    SystemCaller<'a, EvmConfig, Ext, DB, Chainspec, Hook>
{
    /// Installs a custom hook to be called after each state change.
    pub fn with_state_hook<H: OnStateHook>(
        self,
        hook: H,
    ) -> SystemCaller<'a, EvmConfig, Ext, DB, Chainspec, H> {
        let Self { evm_config, db, chain_spec, evm, .. } = self;
        SystemCaller { evm_config, db, chain_spec, evm, hook: Some(hook) }
    }
    /// Convenience method to consume the type and drop borrowed fields
    pub fn finish(self) {}
}

impl<'a, EvmConfig, Ext, DB: Database, Chainspec, Hook>
    SystemCaller<'a, EvmConfig, Ext, DB, Chainspec, Hook>
{
    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn pre_block_blockhashes_contract_call<EXT>(
        mut self,
        initialized_block_env: &BlockEnv,
        parent_block_hash: B256,
    ) -> Result<Self, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let result_and_state = eip2935::transact_blockhashes_contract_call(
            &self.evm_config,
            self.chain_spec.as_ref(),
            initialized_block_env.timestamp.to(),
            initialized_block_env.number.to(),
            parent_block_hash,
            &mut self.evm,
        )?;

        if let Some(res) = result_and_state {
            if let Some(ref mut hook) = self.hook {
                hook.on_state(&res);
            }
            self.evm.context.evm.db.commit(res.state);
        }

        Ok(self)
    }

    // TODO add other system calls
}
