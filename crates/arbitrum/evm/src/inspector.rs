use alloy_evm::eth::EthEvmContext;
use alloy_primitives::{Address, U256};
use revm::{
    context_interface::result::{ExecutionResult, HaltReason, Output, ResultAndState},
    inspector::Inspector,
    primitives::Bytes,
};
use std::sync::{Arc, Mutex};

use crate::hooks::{ArbHooks, ArbStartTxContext};
use crate::TxState;

#[derive(Debug, Clone)]
pub struct ArbHookInspector<CS> {
    pub hooks: Arc<Mutex<ArbHooks<CS>>>,
    pub tx_state: Arc<Mutex<TxState>>,
    pub start_ctx: Option<ArbStartTxContext>,
    pub end_tx_now: bool,
    pub hook_gas_used: u64,
}

impl<CS> ArbHookInspector<CS> {
    pub fn new(
        hooks: Arc<Mutex<ArbHooks<CS>>>,
        tx_state: Arc<Mutex<TxState>>,
        start_ctx: ArbStartTxContext,
    ) -> Self {
        Self {
            hooks,
            tx_state,
            start_ctx: Some(start_ctx),
            end_tx_now: false,
            hook_gas_used: 0,
        }
    }

    pub fn should_end_tx(&self) -> bool {
        self.end_tx_now
    }

    pub fn hook_gas_used(&self) -> u64 {
        self.hook_gas_used
    }
}

impl<CS, DB> Inspector<EthEvmContext<DB>> for ArbHookInspector<CS>
where
    CS: 'static,
    DB: revm::Database,
{
    fn initialize_interp(
        &mut self,
        _interp: &mut revm::interpreter::Interpreter,
        context: &mut EthEvmContext<DB>,
    ) {
        if let Some(start_ctx) = self.start_ctx.take() {
            let mut hooks = self.hooks.lock().unwrap();
            let mut state = self.tx_state.lock().unwrap();
            
            let db = &mut context.db;
            let result = hooks.start_tx(db, &mut state, &start_ctx);
            
            self.end_tx_now = result.end_tx_now;
            self.hook_gas_used = result.gas_used;
            
            if result.end_tx_now {
                tracing::debug!(
                    target: "arb-reth::inspector",
                    gas_used = result.gas_used,
                    error = ?result.error,
                    "StartTxHook requested early transaction end"
                );
            }
        }
    }

    fn step(
        &mut self,
        _interp: &mut revm::interpreter::Interpreter,
        _context: &mut EthEvmContext<DB>,
    ) {
        if self.end_tx_now {
        }
    }
}
