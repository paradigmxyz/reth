//! System contract call functions.

use crate::ConfigureEvm;
use alloc::{boxed::Box, sync::Arc};
use alloy_consensus::Header;
use alloy_eips::{
    eip7002::WITHDRAWAL_REQUEST_TYPE, eip7251::CONSOLIDATION_REQUEST_TYPE, eip7685::Requests,
};
use alloy_primitives::Bytes;
use core::fmt::Display;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::BlockExecutionError;
use reth_primitives::Block;
use revm::{Database, DatabaseCommit, Evm};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, EvmState, B256};

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

fn initialize_evm<'a, DB>(
    db: &'a mut DB,
    initialized_cfg: &'a CfgEnvWithHandlerCfg,
    initialized_block_env: &'a BlockEnv,
) -> Evm<'a, (), &'a mut DB>
where
    DB: Database,
{
    Evm::builder()
        .with_db(db)
        .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            Default::default(),
        ))
        .build()
}

impl<EvmConfig, Chainspec> SystemCaller<EvmConfig, Chainspec>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Chainspec: EthereumHardforks,
{
    /// Apply pre execution changes.
    pub fn apply_pre_execution_changes<DB, Ext>(
        &mut self,
        block: &Block,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        self.apply_blockhashes_contract_call(
            block.timestamp,
            block.number,
            block.parent_hash,
            evm,
        )?;
        self.apply_beacon_root_contract_call(
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            evm,
        )?;

        Ok(())
    }

    /// Apply post execution changes.
    pub fn apply_post_execution_changes<DB, Ext>(
        &mut self,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<Requests, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut requests = Requests::default();

        // Collect all EIP-7685 requests
        let withdrawal_requests = self.apply_withdrawal_requests_contract_call(evm)?;
        if !withdrawal_requests.is_empty() {
            requests.push_request(
                core::iter::once(WITHDRAWAL_REQUEST_TYPE).chain(withdrawal_requests).collect(),
            );
        }

        // Collect all EIP-7251 requests
        let consolidation_requests = self.apply_consolidation_requests_contract_call(evm)?;
        if !consolidation_requests.is_empty() {
            requests.push_request(
                core::iter::once(CONSOLIDATION_REQUEST_TYPE)
                    .chain(consolidation_requests)
                    .collect(),
            );
        }

        Ok(requests)
    }

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn pre_block_blockhashes_contract_call<DB>(
        &mut self,
        db: &mut DB,
        initialized_cfg: &CfgEnvWithHandlerCfg,
        initialized_block_env: &BlockEnv,
        parent_block_hash: B256,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut evm = initialize_evm(db, initialized_cfg, initialized_block_env);
        self.apply_blockhashes_contract_call(
            initialized_block_env.timestamp.to(),
            initialized_block_env.number.to(),
            parent_block_hash,
            &mut evm,
        )?;

        Ok(())
    }

    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub fn apply_blockhashes_contract_call<DB, Ext>(
        &mut self,
        timestamp: u64,
        block_number: u64,
        parent_block_hash: B256,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let result_and_state = eip2935::transact_blockhashes_contract_call(
            &self.evm_config,
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
            evm.context.evm.db.commit(res.state);
        }

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn pre_block_beacon_root_contract_call<DB>(
        &mut self,
        db: &mut DB,
        initialized_cfg: &CfgEnvWithHandlerCfg,
        initialized_block_env: &BlockEnv,
        parent_beacon_block_root: Option<B256>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut evm = initialize_evm(db, initialized_cfg, initialized_block_env);

        self.apply_beacon_root_contract_call(
            initialized_block_env.timestamp.to(),
            initialized_block_env.number.to(),
            parent_beacon_block_root,
            &mut evm,
        )?;

        Ok(())
    }

    /// Applies the pre-block call to the EIP-4788 beacon root contract.
    pub fn apply_beacon_root_contract_call<DB, Ext>(
        &mut self,
        timestamp: u64,
        block_number: u64,
        parent_block_hash: Option<B256>,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<(), BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let result_and_state = eip4788::transact_beacon_root_contract_call(
            &self.evm_config,
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
            evm.context.evm.db.commit(res.state);
        }

        Ok(())
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn post_block_withdrawal_requests_contract_call<DB>(
        &mut self,
        db: &mut DB,
        initialized_cfg: &CfgEnvWithHandlerCfg,
        initialized_block_env: &BlockEnv,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut evm = initialize_evm(db, initialized_cfg, initialized_block_env);

        let result = self.apply_withdrawal_requests_contract_call(&mut evm)?;

        Ok(result)
    }

    /// Applies the post-block call to the EIP-7002 withdrawal request contract.
    pub fn apply_withdrawal_requests_contract_call<DB, Ext>(
        &mut self,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let result_and_state =
            eip7002::transact_withdrawal_requests_contract_call(&self.evm_config.clone(), evm)?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state.state);
        }
        evm.context.evm.db.commit(result_and_state.state);

        eip7002::post_commit(result_and_state.result)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn post_block_consolidation_requests_contract_call<DB>(
        &mut self,
        db: &mut DB,
        initialized_cfg: &CfgEnvWithHandlerCfg,
        initialized_block_env: &BlockEnv,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut evm = initialize_evm(db, initialized_cfg, initialized_block_env);

        let res = self.apply_consolidation_requests_contract_call(&mut evm)?;

        Ok(res)
    }

    /// Applies the post-block call to the EIP-7251 consolidation requests contract.
    pub fn apply_consolidation_requests_contract_call<DB, Ext>(
        &mut self,
        evm: &mut Evm<'_, Ext, DB>,
    ) -> Result<Bytes, BlockExecutionError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let result_and_state =
            eip7251::transact_consolidation_requests_contract_call(&self.evm_config.clone(), evm)?;

        if let Some(ref mut hook) = self.hook {
            hook.on_state(&result_and_state.state);
        }
        evm.context.evm.db.commit(result_and_state.state);

        eip7251::post_commit(result_and_state.result)
    }

    /// Delegate to stored `OnStateHook`, noop if hook is `None`.
    pub fn on_state(&mut self, state: &EvmState) {
        if let Some(ref mut hook) = &mut self.hook {
            hook.on_state(state);
        }
    }
}
