//! System contract call functions.

use crate::Evm;
use alloc::sync::Arc;
use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip7002::WITHDRAWAL_REQUEST_TYPE, eip7251::CONSOLIDATION_REQUEST_TYPE, eip7685::Requests,
};
use alloy_primitives::Bytes;
use core::fmt::Display;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::BlockExecutionError;
use revm::DatabaseCommit;
use revm_primitives::{EvmState, ResultAndState, B256};

mod eip2935;
mod eip4788;
mod eip7002;
mod eip7251;

/// A hook that is called after each state change.
pub trait OnStateHook: Send + Sync {
    /// Invoked with the state after each system call.
    fn on_state(&mut self, state: &EvmState);
}

impl<F> OnStateHook for F
where
    F: Send + Sync + FnMut(&EvmState),
{
    fn on_state(&mut self, state: &EvmState) {
        self(state)
    }
}

/// An [`OnStateHook`] that does nothing.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct NoopHook;

impl OnStateHook for NoopHook {
    fn on_state(&mut self, _state: &EvmState) {}
}

/// Wrapper around [`Evm`], invoking [`OnStateHook`] on every [`Evm::transact`] call.
#[derive(derive_more::Debug)]
pub struct EvmWithHook<Evm> {
    inner: Evm,
    /// Configured [`OnStateHook`], if any.
    #[debug(skip)]
    pub hook: Option<Box<dyn OnStateHook>>,
}

impl<Evm> EvmWithHook<Evm> {
    /// Creates a new instance.
    pub fn new(inner: Evm) -> Self {
        Self { inner, hook: None }
    }
}

impl<E: Evm> Evm for EvmWithHook<E> {
    type DB = E::DB;
    type Error = E::Error;
    type HaltReason = E::HaltReason;
    type Tx = E::Tx;

    fn block(&self) -> &revm_primitives::BlockEnv {
        self.inner.block()
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        self.inner.db_mut()
    }

    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState, Self::Error> {
        let result = self.inner.transact(tx)?;
        if let Some(hook) = &mut self.hook {
            hook.on_state(&result.state);
        }
        Ok(result)
    }

    fn transact_system_call(
        &mut self,
        caller: revm_primitives::Address,
        contract: revm_primitives::Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        let result = self.inner.transact_system_call(caller, contract, data)?;
        if let Some(hook) = &mut self.hook {
            hook.on_state(&result.state);
        }
        Ok(result)
    }
}

/// An ephemeral helper type for executing system calls.
///
/// This can be used to chain system transaction calls.
#[derive(Debug, Clone)]
pub struct SystemCaller<Chainspec> {
    chain_spec: Arc<Chainspec>,
}

impl<Chainspec> SystemCaller<Chainspec> {
    /// Create a new system caller with the given EVM config, database, and chain spec, and creates
    /// the EVM with the given initialized config and block environment.
    pub const fn new(chain_spec: Arc<Chainspec>) -> Self {
        Self { chain_spec }
    }

    /// Convenience method to consume the type and drop borrowed fields
    pub fn finish(self) {}
}

impl<Chainspec> SystemCaller<Chainspec>
where
    Chainspec: EthereumHardforks,
{
    /// Apply pre execution changes.
    pub fn apply_pre_execution_changes(
        &self,
        header: impl BlockHeader,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        self.apply_blockhashes_contract_call(header.parent_hash(), evm)?;
        self.apply_beacon_root_contract_call(header.parent_beacon_block_root(), evm)?;

        Ok(())
    }

    /// Apply post execution changes.
    pub fn apply_post_execution_changes(
        &self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Requests, BlockExecutionError> {
        let mut requests = Requests::default();

        // Collect all EIP-7685 requests
        let withdrawal_requests = self.apply_withdrawal_requests_contract_call(evm)?;
        if !withdrawal_requests.is_empty() {
            requests.push_request_with_type(WITHDRAWAL_REQUEST_TYPE, withdrawal_requests);
        }

        // Collect all EIP-7251 requests
        let consolidation_requests = self.apply_consolidation_requests_contract_call(evm)?;
        if !consolidation_requests.is_empty() {
            requests.push_request_with_type(CONSOLIDATION_REQUEST_TYPE, consolidation_requests);
        }

        Ok(requests)
    }

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn apply_blockhashes_contract_call(
        &self,
        parent_block_hash: B256,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state =
            eip2935::transact_blockhashes_contract_call(&self.chain_spec, parent_block_hash, evm)?;

        if let Some(res) = result_and_state {
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn apply_beacon_root_contract_call(
        &self,
        parent_beacon_block_root: Option<B256>,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state = eip4788::transact_beacon_root_contract_call(
            &self.chain_spec,
            parent_beacon_block_root,
            evm,
        )?;

        if let Some(res) = result_and_state {
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn apply_withdrawal_requests_contract_call(
        &self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7002::transact_withdrawal_requests_contract_call(evm)?;
        evm.db_mut().commit(result_and_state.state);

        eip7002::post_commit(result_and_state.result)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn apply_consolidation_requests_contract_call(
        &self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7251::transact_consolidation_requests_contract_call(evm)?;
        evm.db_mut().commit(result_and_state.state);

        eip7251::post_commit(result_and_state.result)
    }
}
