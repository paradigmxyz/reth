//! System contract call functions.

use crate::{ConfigureEvm, Evm, EvmEnv};
use alloc::{boxed::Box, sync::Arc};
use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip7002::WITHDRAWAL_REQUEST_TYPE, eip7251::CONSOLIDATION_REQUEST_TYPE, eip7685::Requests,
};
use alloy_primitives::Bytes;
use core::fmt::Display;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::BlockExecutionError;
use revm::{Database, DatabaseCommit};
use revm_primitives::{EvmState, B256};

mod eip2935;
mod eip4788;
mod eip7002;
mod eip7251;

/// A hook that is called after each state change.
pub trait OnStateHook {
    /// Invoked with the state after each system call.
    fn on_state(&mut self, state: &EvmState);
}

impl<F> OnStateHook for F
where
    F: FnMut(&EvmState),
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

/// An ephemeral helper type for executing system calls.
///
/// This can be used to chain system transaction calls.
#[allow(missing_debug_implementations)]
pub struct SystemCaller<EvmConfig, Chainspec> {
    evm_config: EvmConfig,
    chain_spec: Arc<Chainspec>,
    /// Optional hook to be called after each state change.
    hook: Option<Box<dyn OnStateHook>>,
}

impl<EvmConfig, Chainspec> SystemCaller<EvmConfig, Chainspec> {
    /// Create a new system caller with the given EVM config, database, and chain spec, and creates
    /// the EVM with the given initialized config and block environment.
    pub const fn new(evm_config: EvmConfig, chain_spec: Arc<Chainspec>) -> Self {
        Self { evm_config, chain_spec, hook: None }
    }

    /// Installs a custom hook to be called after each state change.
    pub fn with_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) -> &mut Self {
        self.hook = hook;
        self
    }

    /// Convenience method to consume the type and drop borrowed fields
    pub fn finish(self) {}
}

impl<EvmConfig, Chainspec> SystemCaller<EvmConfig, Chainspec>
where
    EvmConfig: ConfigureEvm,
    Chainspec: EthereumHardforks,
{
    /// Apply pre execution changes.
    pub fn apply_pre_execution_changes(
        &mut self,
        header: &EvmConfig::Header,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        self.apply_blockhashes_contract_call(
            header.timestamp(),
            header.number(),
            header.parent_hash(),
            evm,
        )?;
        self.apply_beacon_root_contract_call(
            header.timestamp(),
            header.number(),
            header.parent_beacon_block_root(),
            evm,
        )?;

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
    pub fn pre_block_blockhashes_contract_call<DB>(
        &mut self,
        db: &mut DB,
        evm_env: &EvmEnv<EvmConfig::Spec>,
        parent_block_hash: B256,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let evm_config = self.evm_config.clone();
        let mut evm = evm_config.evm_with_env(db, evm_env.clone());

        self.apply_blockhashes_contract_call(
            evm_env.block_env.timestamp.to(),
            evm_env.block_env.number.to(),
            parent_block_hash,
            &mut evm,
        )?;

        Ok(())
    }

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn apply_blockhashes_contract_call(
        &mut self,
        timestamp: u64,
        block_number: u64,
        parent_block_hash: B256,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state = eip2935::transact_blockhashes_contract_call(
            &self.chain_spec,
            timestamp,
            block_number,
            parent_block_hash,
            evm,
        )?;

        if let Some(res) = result_and_state {
            if let Some(ref mut hook) = self.hook {
                hook.on_state(&res.state);
            }
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn pre_block_beacon_root_contract_call<DB>(
        &mut self,
        db: &mut DB,
        evm_env: &EvmEnv<EvmConfig::Spec>,
        parent_beacon_block_root: Option<B256>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let evm_config = self.evm_config.clone();
        let mut evm = evm_config.evm_with_env(db, evm_env.clone());

        self.apply_beacon_root_contract_call(
            evm_env.block_env.timestamp.to(),
            evm_env.block_env.number.to(),
            parent_beacon_block_root,
            &mut evm,
        )?;

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn apply_beacon_root_contract_call(
        &mut self,
        timestamp: u64,
        block_number: u64,
        parent_block_hash: Option<B256>,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state = eip4788::transact_beacon_root_contract_call(
            &self.chain_spec,
            timestamp,
            block_number,
            parent_block_hash,
            evm,
        )?;

        if let Some(res) = result_and_state {
            if let Some(ref mut hook) = self.hook {
                hook.on_state(&res.state);
            }
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn post_block_withdrawal_requests_contract_call<DB>(
        &mut self,
        db: &mut DB,
        evm_env: &EvmEnv<EvmConfig::Spec>,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let evm_config = self.evm_config.clone();
        let mut evm = evm_config.evm_with_env(db, evm_env.clone());

        let result = self.apply_withdrawal_requests_contract_call(&mut evm)?;

        Ok(result)
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn apply_withdrawal_requests_contract_call(
        &mut self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7002::transact_withdrawal_requests_contract_call(evm)?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state.state);
        }
        evm.db_mut().commit(result_and_state.state);

        eip7002::post_commit(result_and_state.result)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn post_block_consolidation_requests_contract_call<DB>(
        &mut self,
        db: &mut DB,
        evm_env: &EvmEnv<EvmConfig::Spec>,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let evm_config = self.evm_config.clone();
        let mut evm = evm_config.evm_with_env(db, evm_env.clone());

        let res = self.apply_consolidation_requests_contract_call(&mut evm)?;

        Ok(res)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn apply_consolidation_requests_contract_call(
        &mut self,
        evm: &mut impl Evm<DB: DatabaseCommit, Error: Display>,
    ) -> Result<Bytes, BlockExecutionError> {
        let result_and_state = eip7251::transact_consolidation_requests_contract_call(evm)?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state.state);
        }
        evm.db_mut().commit(result_and_state.state);

        eip7251::post_commit(result_and_state.result)
    }

    /// Delegate to stored `OnStateHook`, noop if hook is `None`.
    pub fn on_state(&mut self, state: &EvmState) {
        if let Some(ref mut hook) = &mut self.hook {
            hook.on_state(state);
        }
    }
}
