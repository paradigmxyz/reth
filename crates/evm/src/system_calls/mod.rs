//! System contract call functions.

use crate::ConfigureEvm;
use alloc::{format, string::ToString, vec::Vec};
use alloy_eips::{eip7002::WithdrawalRequest, eip7251::ConsolidationRequest};
use alloy_primitives::{Address, FixedBytes};
use core::fmt::Display;
use reth_chainspec::ChainSpec;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{Buf, Header, Request};
use revm::{Database, DatabaseCommit, Evm};
use revm_primitives::{
    BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, ResultAndState, B256,
};

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
pub struct SystemCaller<'a, EvmConfig, DB: Database, Chainspec, Hook = NoopHook> {
    evm: Evm<'a, (), DB>,
    evm_config: EvmConfig,
    chain_spec: Chainspec,
    /// Optional hook to be called after each state change.
    // TODO do we want this optional?
    hook: Option<Hook>,
}

impl<'a, EvmConfig, DB: Database, Chainspec> SystemCaller<'a, EvmConfig, DB, Chainspec> {
    /// Create a new system caller with the given EVM, EVM config, database, and chain spec.
    pub const fn new_with_evm(
        evm_config: EvmConfig,
        chain_spec: Chainspec,
        evm: Evm<'a, (), DB>,
    ) -> Self {
        Self { evm_config, chain_spec, evm, hook: None }
    }
    /// Create a new system caller with the given EVM config, database, and chain spec, and creates
    /// the EVM with the given initialized config and block environment.
    pub fn new_with_initialized_config_and_block_env(
        evm_config: EvmConfig,
        db: DB,
        chain_spec: Chainspec,
        initialized_cfg: &CfgEnvWithHandlerCfg,
        initialized_block_env: &BlockEnv,
    ) -> Self {
        let evm = Evm::builder()
            .with_db(db)
            .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
                initialized_cfg.clone(),
                initialized_block_env.clone(),
                Default::default(),
            ))
            .build();

        Self { evm_config, chain_spec, evm, hook: None }
    }
}

impl<'a, EvmConfig, DB: Database, Chainspec, Hook>
    SystemCaller<'a, EvmConfig, DB, Chainspec, Hook>
{
    /// Installs a custom hook to be called after each state change.
    pub fn with_state_hook<H: OnStateHook>(
        self,
        hook: H,
    ) -> SystemCaller<'a, EvmConfig, DB, Chainspec, H> {
        let Self { evm_config, chain_spec, evm, .. } = self;
        SystemCaller { evm_config, chain_spec, evm, hook: Some(hook) }
    }
    /// Convenience method to consume the type and drop borrowed fields
    pub fn finish(self) {}
}

impl<'a, EvmConfig, DB: Database, Chainspec, Hook>
    SystemCaller<'a, EvmConfig, DB, Chainspec, Hook>
{
    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn pre_block_blockhashes_contract_call(
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
        self.apply_blockhashes_contract_call(initialized_block_env, parent_block_hash)?;

        Ok(self)
    }

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn apply_blockhashes_contract_call(
        &mut self,
        initialized_block_env: &BlockEnv,
        parent_block_hash: B256,
    ) -> Result<(), BlockExecutionError>
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

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn pre_block_beacon_root_contract_call(
        mut self,
        initialized_block_env: &BlockEnv,
        parent_block_hash: Option<B256>,
    ) -> Result<Self, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        self.apply_beacon_root_contract_call(initialized_block_env, parent_block_hash)?;

        Ok(self)
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn apply_beacon_root_contract_call(
        &mut self,
        initialized_block_env: &BlockEnv,
        parent_block_hash: Option<B256>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let result_and_state = eip4788::transact_beacon_root_contract_call(
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

        Ok(())
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn post_block_withdrawal_request_contract_call(
        self,
    ) -> Result<Vec<Request>, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let result = self.apply_withdrawal_request_contract_call()?;

        Ok(result)
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn apply_withdrawal_request_contract_call(
        mut self,
    ) -> Result<Vec<Request>, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let result_and_state =
            eip7002::transact_withdrawal_requests_contract_call(&self.evm_config, &mut self.evm)?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state);
        }
        self.evm.context.evm.db.commit(result_and_state.state);

        let mut data = match result_and_state.result {
            ExecutionResult::Success { output, .. } => Ok(output.into_data()),
            ExecutionResult::Revert { output, .. } => {
                Err(BlockValidationError::WithdrawalRequestsContractCall {
                    message: format!("execution reverted: {output}"),
                })
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(BlockValidationError::WithdrawalRequestsContractCall {
                    message: format!("execution halted: {reason:?}"),
                })
            }
        }?;

        // Withdrawals are encoded as a series of withdrawal requests, each with the following
        // format:
        //
        // +------+--------+--------+
        // | addr | pubkey | amount |
        // +------+--------+--------+
        //    20      48        8

        const WITHDRAWAL_REQUEST_SIZE: usize = 20 + 48 + 8;
        let mut withdrawal_requests = Vec::with_capacity(data.len() / WITHDRAWAL_REQUEST_SIZE);
        while data.has_remaining() {
            if data.remaining() < WITHDRAWAL_REQUEST_SIZE {
                return Err(BlockValidationError::WithdrawalRequestsContractCall {
                    message: "invalid withdrawal request length".to_string(),
                }
                .into())
            }

            let mut source_address = Address::ZERO;
            data.copy_to_slice(source_address.as_mut_slice());

            let mut validator_pubkey = FixedBytes::<48>::ZERO;
            data.copy_to_slice(validator_pubkey.as_mut_slice());

            let amount = data.get_u64();

            withdrawal_requests
                .push(WithdrawalRequest { source_address, validator_pubkey, amount }.into());
        }

        Ok(withdrawal_requests)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn post_block_consolidation_requests_contract_call(
        self,
    ) -> Result<Vec<Request>, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let res = self.apply_consolidation_requests_contract_call()?;

        Ok(res)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn apply_consolidation_requests_contract_call(
        mut self,
    ) -> Result<Vec<Request>, BlockExecutionError>
    where
        DB: Database + DatabaseCommit + Clone,
        DB::Error: Display,
        EvmConfig: ConfigureEvm<Header = Header>,
        Chainspec: AsRef<ChainSpec>,
        Hook: OnStateHook,
    {
        let result_and_state = eip7251::transact_consolidation_requests_contract_call(
            &self.evm_config,
            &mut self.evm,
        )?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state);
        }
        self.evm.context.evm.db.commit(result_and_state.state);

        let mut data = match result_and_state.result {
            ExecutionResult::Success { output, .. } => Ok(output.into_data()),
            ExecutionResult::Revert { output, .. } => {
                Err(BlockValidationError::ConsolidationRequestsContractCall {
                    message: format!("execution reverted: {output}"),
                })
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(BlockValidationError::ConsolidationRequestsContractCall {
                    message: format!("execution halted: {reason:?}"),
                })
            }
        }?;

        // Consolidations are encoded as a series of consolidation requests, each with the following
        // format:
        //
        // +------+--------+---------------+
        // | addr | pubkey | target pubkey |
        // +------+--------+---------------+
        //    20      48        48

        const CONSOLIDATION_REQUEST_SIZE: usize = 20 + 48 + 48;
        let mut consolidation_requests =
            Vec::with_capacity(data.len() / CONSOLIDATION_REQUEST_SIZE);
        while data.has_remaining() {
            if data.remaining() < CONSOLIDATION_REQUEST_SIZE {
                return Err(BlockValidationError::ConsolidationRequestsContractCall {
                    message: "invalid consolidation request length".to_string(),
                }
                .into())
            }

            let mut source_address = Address::ZERO;
            data.copy_to_slice(source_address.as_mut_slice());

            let mut source_pubkey = FixedBytes::<48>::ZERO;
            data.copy_to_slice(source_pubkey.as_mut_slice());

            let mut target_pubkey = FixedBytes::<48>::ZERO;
            data.copy_to_slice(target_pubkey.as_mut_slice());

            consolidation_requests.push(Request::ConsolidationRequest(ConsolidationRequest {
                source_address,
                source_pubkey,
                target_pubkey,
            }));
        }

        Ok(consolidation_requests)
    }
}
