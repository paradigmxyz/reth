//! System contract call functions.

use crate::Evm;
use alloc::boxed::Box;
use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip7002::WITHDRAWAL_REQUEST_TYPE, eip7251::CONSOLIDATION_REQUEST_TYPE, eip7685::Requests,
};
use alloy_primitives::{Bytes, B256};
use core::fmt::Display;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::BlockExecutionError;
use revm::{state::EvmState, DatabaseCommit};

mod eip2935;
mod eip4788;
mod eip7002;
mod eip7251;

/// A hook that is called after each state change.
pub trait OnStateHook: Send + 'static {
    /// Invoked with the source of the change and the state after each system call.
    fn on_state(&mut self, source: StateChangeSource, state: &EvmState);
}

/// Source of the state change
#[derive(Debug, Clone, Copy)]
pub enum StateChangeSource {
    /// Transaction with its index
    Transaction(usize),
    /// Pre-block state transition
    PreBlock(StateChangePreBlockSource),
    /// Post-block state transition
    PostBlock(StateChangePostBlockSource),
}

/// Source of the pre-block state change
#[derive(Debug, Clone, Copy)]
pub enum StateChangePreBlockSource {
    /// EIP-2935 blockhashes contract
    BlockHashesContract,
    /// EIP-4788 beacon root contract
    BeaconRootContract,
    /// EIP-7002 withdrawal requests contract
    WithdrawalRequestsContract,
}

/// Source of the post-block state change
#[derive(Debug, Clone, Copy)]
pub enum StateChangePostBlockSource {
    /// Balance increments from block rewards and withdrawals
    BalanceIncrements,
    /// EIP-7002 withdrawal requests contract
    WithdrawalRequestsContract,
    /// EIP-7251 consolidation requests contract
    ConsolidationRequestsContract,
}

impl<F> OnStateHook for F
where
    F: FnMut(StateChangeSource, &EvmState) + Send + 'static,
{
    fn on_state(&mut self, source: StateChangeSource, state: &EvmState) {
        self(source, state)
    }
}

/// An [`OnStateHook`] that does nothing.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct NoopHook;

impl OnStateHook for NoopHook {
    fn on_state(&mut self, _source: StateChangeSource, _state: &EvmState) {}
}

/// An ephemeral helper type for executing system calls.
///
/// This can be used to chain system transaction calls.
#[derive(derive_more::Debug)]
pub struct SystemCaller<ChainSpec> {
    chain_spec: ChainSpec,
    /// Optional hook to be called after each state change.
    #[debug(skip)]
    hook: Option<Box<dyn OnStateHook>>,
}

impl<ChainSpec> SystemCaller<ChainSpec> {
    /// Create a new system caller with the given EVM config, database, and chain spec, and creates
    /// the EVM with the given initialized config and block environment.
    pub const fn new(chain_spec: ChainSpec) -> Self {
        Self { chain_spec, hook: None }
    }

    /// Installs a custom hook to be called after each state change.
    pub fn with_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) -> &mut Self {
        self.hook = hook;
        self
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
        &mut self,
        header: impl BlockHeader,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        self.apply_blockhashes_contract_call(header.parent_hash(), evm)?;
        self.apply_beacon_root_contract_call(header.parent_beacon_block_root(), evm)?;

        Ok(())
    }

    /// Apply post execution changes.
    pub fn apply_post_execution_changes(
        &mut self,
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
        &mut self,
        parent_block_hash: B256,
        evm: &mut impl Evm<DB: DatabaseCommit>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state =
            eip2935::transact_blockhashes_contract_call(&self.chain_spec, parent_block_hash, evm)?;

        if let Some(res) = result_and_state {
            if let Some(hook) = &mut self.hook {
                hook.on_state(
                    StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract),
                    &res.state,
                );
            }
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn apply_beacon_root_contract_call(
        &mut self,
        parent_beacon_block_root: Option<B256>,
        evm: &mut impl Evm<DB: DatabaseCommit>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state = eip4788::transact_beacon_root_contract_call(
            &self.chain_spec,
            parent_beacon_block_root,
            evm,
        )?;

        if let Some(res) = result_and_state {
            if let Some(hook) = &mut self.hook {
                hook.on_state(
                    StateChangeSource::PreBlock(StateChangePreBlockSource::BeaconRootContract),
                    &res.state,
                );
            }
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn apply_withdrawal_requests_contract_call(
        &mut self,
        evm: &mut impl Evm<DB: DatabaseCommit>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7002::transact_withdrawal_requests_contract_call(evm)?;

        if let Some(ref mut hook) = &mut self.hook {
            hook.on_state(
                StateChangeSource::PostBlock(
                    StateChangePostBlockSource::WithdrawalRequestsContract,
                ),
                &result_and_state.state,
            );
        }
        evm.db_mut().commit(result_and_state.state);

        eip7002::post_commit(result_and_state.result)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn apply_consolidation_requests_contract_call(
        &mut self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7251::transact_consolidation_requests_contract_call(evm)?;

        if let Some(ref mut hook) = &mut self.hook {
            hook.on_state(
                StateChangeSource::PostBlock(
                    StateChangePostBlockSource::ConsolidationRequestsContract,
                ),
                &result_and_state.state,
            );
        }
        evm.db_mut().commit(result_and_state.state);

        eip7251::post_commit(result_and_state.result)
    }

    /// Delegate to stored `OnStateHook`, noop if hook is `None`.
    pub fn on_state(&mut self, source: StateChangeSource, state: &EvmState) {
        if let Some(hook) = &mut self.hook {
            hook.on_state(source, state);
        }
    }
}
