//! System contract call functions.

mod eip2935;

pub use eip2935::*;
use revm_primitives::ResultAndState;

mod eip4788;
pub use eip4788::*;

mod eip7002;
pub use eip7002::*;

mod eip7251;
pub use eip7251::*;
use reth_execution_errors::BlockExecutionError;

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
#[derive(Debug)]
pub struct SystemCaller<EvmConfig, DB, Chainspec, Hook = NoopHook> {
    evm_config: EvmConfig,
    db: DB,
    chain_spec: Chainspec,
    /// Optional hook to be called after each state change.
    // TODO do we want this optional?
    hook: Option<Hook>,
}

impl<EvmConfig, DB, Chainspec> SystemCaller<EvmConfig, DB, Chainspec> {
    /// Create a new system caller with the given EVM config, database, and chain spec.
    pub fn new(evm_config: EvmConfig, db: DB, chain_spec: Chainspec) -> Self {
        Self { evm_config, db, chain_spec, hook: None }
    }
}

impl<EvmConfig, DB, Chainspec, Hook> SystemCaller<EvmConfig, DB, Chainspec, Hook> {
    /// Installs a custom hook to be called after each state change.
    pub fn with_state_hook<H: OnStateHook>(
        self,
        hook: H,
    ) -> SystemCaller<EvmConfig, DB, Chainspec, H> {
        let Self { evm_config, db, chain_spec, .. } = self;
        SystemCaller { evm_config, db, chain_spec, hook: Some(hook) }
    }
    /// Convenience type to consume the type and drop borrowed fields
    pub fn finish(self) {}
}

impl<EvmConfig, DB, Chainspec, Hook> SystemCaller<EvmConfig, DB, Chainspec, Hook> {
    // TODO add apply functions that are chainable

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn pre_block_blockhashes_contract_call(self) -> Result<Self, BlockExecutionError> {
        // TODO:
        // transact
        // apply hook
        // commit
        todo!()
    }

    // TODO add other system calls
}
