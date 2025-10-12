#![allow(unused)]
extern crate alloc;

use alloc::{boxed::Box, sync::Arc};
use core::marker::PhantomData;
use std::sync::Mutex;
use alloy_consensus::Transaction;
use alloy_evm::{eth::EthEvmContext, precompiles::PrecompilesMap};
use alloy_primitives::{Address, B256};
use reth_evm_ethereum::{EthEvmConfig};
use crate::ArbEvmFactory;
use crate::header;
use std::collections::HashMap;
use revm_state::{AccountInfo as RevmAccountInfo, EvmStorageSlot};
use revm_database::{BundleAccount, AccountStatus};
use revm::{
    context::TxEnv,
    inspector::Inspector,
    interpreter::interpreter::EthInterpreter,
    context::result::{ExecutionResult, ResultAndState},
};
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput, WithTxEnv, ExecutorTx};
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::BlockExecutionResult as RethBlockExecutionResult;
use reth_evm::{OnStateHook, TransactionEnv};
use alloy_evm::Database;
use revm::Database as RevmDatabase;
use alloy_evm::block::{BlockExecutorFactory, BlockExecutorFor, CommitChanges, ExecutableTx, BlockExecutor as AlloyBlockExecutor};
use crate::arb_evm::ArbEvmExt;
use alloy_evm::eth::{EthBlockExecutionCtx, EthBlockExecutor};
use alloy_evm::ToTxEnv;
use alloy_primitives::Log as AlloyLog;

use crate::predeploys::PredeployRegistry;
use crate::predeploys::{PredeployCallContext, LogEmitter};
use crate::execute::{DefaultArbOsHooks, ArbTxProcessorState, ArbStartTxContext, ArbGasChargingContext, ArbEndTxContext, ArbOsHooks};

pub struct ArbBlockExecutorFactory<R, CS>
where
    R: Clone,
{
    receipt_builder: R,
    spec: Arc<CS>,
    predeploys: Arc<Mutex<PredeployRegistry>>,
    evm_factory: ArbEvmFactory,
}

impl<R: Clone, CS> Clone for ArbBlockExecutorFactory<R, CS> {
    fn clone(&self) -> Self {
        Self {
            receipt_builder: self.receipt_builder.clone(),
            spec: self.spec.clone(),
            predeploys: self.predeploys.clone(),
            evm_factory: self.evm_factory.clone(),
        }
    }
}
impl<R: Clone, CS> core::fmt::Debug for ArbBlockExecutorFactory<R, CS> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ArbBlockExecutorFactory").finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ArbBlockExecutionCtx {
    pub parent_hash: B256,
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: alloy_primitives::bytes::Bytes,
    pub delayed_messages_read: u64,
    pub l1_block_number: u64,
}

pub struct ArbBlockExecutor<'a, Evm, CS, RB: alloy_evm::eth::receipt_builder::ReceiptBuilder> {
    inner: EthBlockExecutor<'a, Evm, alloy_evm::eth::spec::EthSpec, &'a RB>,
    predeploys: Arc<Mutex<PredeployRegistry>>,
    hooks: DefaultArbOsHooks,
    tx_state: ArbTxProcessorState,
    _phantom: PhantomData<CS>,
}

impl<R: Clone, CS> ArbBlockExecutorFactory<R, CS> {
    pub fn new(receipt_builder: R, spec: Arc<CS>) -> Self {
        let predeploys = Arc::new(Mutex::new(PredeployRegistry::with_default_addresses()));
        let evm_factory = ArbEvmFactory::default();
        Self { receipt_builder, spec, predeploys, evm_factory }
    }

    pub const fn spec(&self) -> &Arc<CS> {
        &self.spec
    }

    pub fn evm_factory(&self) -> ArbEvmFactory {
        ArbEvmFactory::default()
    }
}

impl<'a, E, CS, RB, D> AlloyBlockExecutor for ArbBlockExecutor<'a, E, CS, RB>
where
    RB: alloy_evm::eth::receipt_builder::ReceiptBuilder<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt>,
    D: RevmDatabase + core::fmt::Debug + 'a,
    <D as RevmDatabase>::Error: Send + Sync + 'static,
    E: reth_evm::Evm<DB = &'a mut revm::database::State<D>> + crate::arb_evm::ArbEvmExt,
    E::Tx: Clone + alloy_evm::tx::FromRecoveredTx<reth_arbitrum_primitives::ArbTransactionSigned> + alloy_evm::tx::FromTxWithEncoded<reth_arbitrum_primitives::ArbTransactionSigned>,
    for<'b> alloy_evm::eth::EthBlockExecutor<'b, E, alloy_evm::eth::spec::EthSpec, &'b RB>: alloy_evm::block::BlockExecutor<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt, Evm = E>,
    <E as alloy_evm::Evm>::Tx: reth_evm::TransactionEnv,
{
    type Transaction = reth_arbitrum_primitives::ArbTransactionSigned;
    type Receipt = reth_arbitrum_primitives::ArbReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.tx_state.brotli_compression_level = 0;
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<ResultAndState<<Self::Evm as reth_evm::Evm>::HaltReason>, BlockExecutionError> {
        self.inner.execute_transaction_without_commit(tx)
    }

    fn commit_transaction(
        &mut self,
        output: ResultAndState<<Self::Evm as reth_evm::Evm>::HaltReason>,
        tx: impl ExecutableTx<Self>,
    ) -> Result<u64, BlockExecutionError> {
        self.inner.commit_transaction(output, tx)
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        let sender = *tx.signer();
        let nonce = tx.tx().nonce();
        let calldata = tx.tx().input().clone();
        let calldata_len = calldata.len();
        let gas_limit = tx.tx().gas_limit();

        let block_env = alloy_evm::Evm::block(self.evm());
        let block_basefee = alloy_primitives::U256::from(block_env.basefee);
        let block_coinbase = block_env.beneficiary;

        let is_sequenced = {
            use reth_arbitrum_primitives::ArbTxType::*;
            match tx.tx().tx_type() {
                Deposit | Internal => false,
                _ => true,
            }
        };
        let is_internal = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Internal);
        let is_deposit = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Deposit);
        let needs_precredit = is_sequenced;

        let paid_gas_price = {
            use reth_arbitrum_primitives::ArbTxType::*;
            match tx.tx().tx_type() {
                Legacy => block_basefee,
                Deposit | Internal => block_basefee,
                _ => alloy_primitives::U256::from(tx.tx().max_fee_per_gas()),
            }
        };
        let upfront_gas_price = alloy_primitives::U256::from(tx.tx().max_fee_per_gas());

        let tx_type_u8 = match tx.tx().tx_type() {
            ArbTxType::Deposit => 0x64,
            ArbTxType::Unsigned => 0x65,
            ArbTxType::Contract => 0x66,
            ArbTxType::Retry => 0x68,
            ArbTxType::SubmitRetryable => 0x69,
            ArbTxType::Internal => 0x6A,
            ArbTxType::Legacy => 0x78,
            ArbTxType::Eip2930 => 0x01,
            ArbTxType::Eip1559 => 0x02,
            ArbTxType::Eip4844 => 0x03,
            ArbTxType::Eip7702 => 0x04,
        };
        
        let to_addr_opt = match tx.tx().kind() {
            alloy_primitives::TxKind::Call(a) => Some(a),
            _ => None,
        };
        
        let block_timestamp = alloy_evm::Evm::block(self.evm()).timestamp.try_into().unwrap_or(0);
        
        let tx_hash = {
            use alloy_eips::eip2718::Encodable2718;
            let mut buf = Vec::new();
            tx.tx().encode_2718(&mut buf);
            alloy_primitives::keccak256(&buf)
        };
        
        let (ticket_id, refund_to, gas_fee_cap_opt, max_refund, submission_fee_refund, deposit_value, retry_value, retry_to, retry_data, beneficiary, max_submission_fee, fee_refund_addr) = match &**tx.tx() {
            reth_arbitrum_primitives::ArbTypedTransaction::Retry(retry_tx) => {
                (Some(retry_tx.ticket_id), Some(retry_tx.refund_to), Some(retry_tx.gas_fee_cap), Some(retry_tx.max_refund), Some(retry_tx.submission_fee_refund), None, None, None, None, None, None, None)
            },
            reth_arbitrum_primitives::ArbTypedTransaction::SubmitRetryable(submit_tx) => {
                (None, None, Some(submit_tx.gas_fee_cap), None, None, Some(submit_tx.deposit_value), Some(submit_tx.retry_value), submit_tx.retry_to, Some(submit_tx.retry_data.to_vec()), Some(submit_tx.beneficiary), Some(submit_tx.max_submission_fee), Some(submit_tx.fee_refund_addr))
            },
            _ => (None, None, None, None, None, None, None, None, None, None, None, None),
        };
        
        let start_ctx = ArbStartTxContext {
            sender,
            nonce,
            l1_base_fee: block_basefee,
            calldata_len,
            coinbase: block_coinbase,
            executed_on_chain: true,
            is_eth_call: false,
            tx_type: tx_type_u8,
            to: to_addr_opt,
            value: tx.tx().value(),
            gas_limit,
            basefee: block_basefee,
            ticket_id,
            refund_to,
            gas_fee_cap: gas_fee_cap_opt,
            max_refund,
            submission_fee_refund,
            tx_hash,
            deposit_value,
            retry_value,
            retry_to,
            retry_data,
            beneficiary,
            max_submission_fee,
            fee_refund_addr,
            block_timestamp,
        };
        
        let start_hook_result = {
            let mut state = core::mem::take(&mut self.tx_state);
            let result = {
                let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
                let state_db: &mut revm::database::State<D> = *db_ref;
                self.hooks.start_tx(state_db, &mut state, &start_ctx)
            };
            self.tx_state = state;
            result
        };

        if start_hook_result.end_tx_now {
            tracing::debug!(
                target: "arb-reth::executor",
                tx_type = ?tx.tx().tx_type(),
                gas_used = start_hook_result.gas_used,
                error = ?start_hook_result.error,
                "Transaction ended early by start_tx hook"
            );
            
            if let Some(err_msg) = start_hook_result.error {
                return Err(BlockExecutionError::msg(err_msg));
            }
            
            return Ok(Some(start_hook_result.gas_used));
        }

        let tx_type = tx.tx().tx_type();
        use reth_arbitrum_primitives::ArbTxType;

        let tx_bytes = {
            use alloy_eips::eip2718::Encodable2718;
            let mut buf = Vec::new();
            tx.tx().encode_2718(&mut buf);
            buf
        };
        
        let gas_ctx = ArbGasChargingContext {
            intrinsic_gas: 21_000,
            calldata: tx_bytes,
            basefee: block_basefee,
            is_executed_on_chain: true,
            skip_l1_charging: false,
        };
        {
            let mut state = core::mem::take(&mut self.tx_state);
            let res = {
                let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
                let state_db: &mut revm::database::State<D> = *db_ref;
                self.hooks.gas_charging(state_db, &mut state, &gas_ctx)
            };
            self.tx_state = state;
            let _ = res;
        }

        let to_addr = match tx.tx().kind() {
            alloy_primitives::TxKind::Call(a) => Some(a),
            _ => None,
        };

        if is_deposit {
            let deposit_value = tx.tx().value();
            if !deposit_value.is_zero() {
                if let Some(to) = to_addr {
                    let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
                    let db: &mut revm::database::State<D> = *db_ref;
                    let _ = DefaultArbOsHooks::execute_deposit(db, sender, to, deposit_value);
                }
            }
        }

        let mut used_pre_nonce = None;
        let mut maybe_predeploy_result: Option<(revm::context::result::ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>, u64)> = None;
        if let Some(call_to) = to_addr {
            let evm = self.inner.evm();
            let block = alloy_evm::Evm::block(evm);
            let ctx = PredeployCallContext {
                block_number: u64::try_from(block.number).unwrap_or(0),
                block_hashes: alloc::vec::Vec::new(),
                chain_id: alloy_primitives::U256::from(self.inner.evm().chain_id()),
                os_version: 0,
                time: u64::try_from(block.timestamp).unwrap_or(0),
                origin: sender,
                caller: sender,
                depth: 0,
                basefee: block_basefee,
            };
            crate::log_sink::clear();
            struct SinkEmitter;
            impl LogEmitter for SinkEmitter {
                fn emit_log(&mut self, address: alloy_primitives::Address, topics: &[[u8; 32]], data: &[u8]) {
                    crate::log_sink::push(address, topics, data);
                }
            }
            let mut emitter = SinkEmitter;
            
            use crate::retryables::DefaultRetryables;
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let db: &mut revm::database::State<D> = *db_ref;
            
            let arbos_addr = alloy_primitives::Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                                              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                                              0x00, 0x00, 0x00, 0x64]);
            let _ = db.basic(arbos_addr);
            
            let mut retryables = DefaultRetryables::new(db as *mut _, alloy_primitives::B256::ZERO);
            
            if let Ok(mut reg) = self.predeploys.lock() {
                let _ = reg.dispatch_with_emitter(&ctx, call_to, &calldata, gas_limit, alloy_primitives::U256::from(tx.tx().value()), &mut retryables, &mut emitter);
            }
        }

        let current_nonce = {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            match state.basic(sender) {
                Ok(info_opt) => info_opt.map(|i| i.nonce).unwrap_or_default(),
                Err(_) => 0,
            }
        };

        if is_internal {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state_db: &mut revm::database::State<D> = *db_ref;
            
            let mut tx_state = core::mem::take(&mut self.tx_state);
            
            if let Err(e) = crate::internal_tx::apply_internal_tx_update(state_db, &mut tx_state, tx.tx()) {
                tracing::error!(target: "arb-reth::executor", error = %e, "Failed to apply internal tx update");
            } else {
                tracing::info!(target: "arb-reth::executor", "Successfully applied internal tx update");
            }
            
            self.tx_state = tx_state;
        }

        let mut tx_env = tx.to_tx_env();
        if is_internal {
            reth_evm::TransactionEnv::set_gas_price(&mut tx_env, block_basefee.to::<u128>());
        } else if is_deposit {
            reth_evm::TransactionEnv::set_gas_price(&mut tx_env, block_basefee.to::<u128>());
        }
        if is_internal || is_deposit {
            reth_evm::TransactionEnv::set_nonce(&mut tx_env, current_nonce);
        }

        if needs_precredit {
            if is_sequenced {
                used_pre_nonce = Some(current_nonce);
            }

            let mut effective_gas_limit = gas_limit;
            if (is_internal || is_deposit) && gas_limit == 0 {
                effective_gas_limit = 1_000_000;
            }
            let effective_gas_price = if is_internal || is_deposit {
                block_basefee
            } else {
                upfront_gas_price
            };
            let needed_fee = alloy_primitives::U256::from(effective_gas_limit) * effective_gas_price;
            tracing::info!(
                target: "arb-reth::executor",
                tx_type = ?tx.tx().tx_type(),
                is_sequenced = is_sequenced,
                gas_limit = effective_gas_limit,
                paid_gas_price = %paid_gas_price,
                upfront_gas_price = %upfront_gas_price,
                needed_fee = %needed_fee,
                "pre-crediting sender for upfront funds"
            );

            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            let inc: u128 = needed_fee.to::<u128>();
            let _ = state.increment_balances(core::iter::once((sender, inc)));
        }






        if let Some(pre_nonce) = used_pre_nonce {
            reth_evm::TransactionEnv::set_nonce(&mut tx_env, pre_nonce);
        }

        let evm = self.inner.evm_mut();
        let prev_disable = evm.cfg_mut().disable_balance_check;
        evm.cfg_mut().disable_balance_check = is_internal || is_deposit;

        let wrapped = WithTxEnv { tx_env, tx };
        let result = self.inner.execute_transaction_with_commit_condition(wrapped, f);

        let evm = self.inner.evm_mut();
        evm.cfg_mut().disable_balance_check = prev_disable;

        if used_pre_nonce.is_some() {
            if let Ok(Some(_)) = result {
                let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
                let state: &mut revm::database::State<D> = *db_ref;
                if let Some(acc) = state.bundle_state.state.get_mut(&sender) {
                    if let Some(info) = acc.info.as_mut() {
                        if info.nonce > 0 {
                            info.nonce -= 1;
                        }
                    }
                }
            }
        }

        let tx_type_u8 = match tx_type {
            ArbTxType::Deposit => 0x64,
            ArbTxType::Unsigned => 0x65,
            ArbTxType::Contract => 0x66,
            ArbTxType::Retry => 0x68,
            ArbTxType::SubmitRetryable => 0x69,
            ArbTxType::Internal => 0x6A,
            ArbTxType::Legacy => 0x78,
            ArbTxType::Eip2930 => 0x01,
            ArbTxType::Eip1559 => 0x02,
            ArbTxType::Eip4844 => 0x03,
            ArbTxType::Eip7702 => 0x04,
        };
        
        let end_ctx = ArbEndTxContext {
            success: result.is_ok(),
            gas_left: 0,
            gas_limit,
            basefee: block_basefee,
            tx_type: tx_type_u8,
        };
        {
            let mut state = core::mem::take(&mut self.tx_state);
            {
                let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
                let state_db: &mut revm::database::State<D> = *db_ref;
                self.hooks.end_tx(state_db, &mut state, &end_ctx);
            }
            self.tx_state = state;
        }

        result
    }

    fn finish(self) -> Result<(Self::Evm, RethBlockExecutionResult<reth_arbitrum_primitives::ArbReceipt>), BlockExecutionError> {
        self.inner.finish()
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }
}

impl<R, CS> BlockExecutorFactory for ArbBlockExecutorFactory<R, CS>
where
    R: Clone + 'static + alloy_evm::eth::receipt_builder::ReceiptBuilder<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt>,
    CS: 'static,
{
    type EvmFactory = ArbEvmFactory;
    type ExecutionCtx<'a> = ArbBlockExecutionCtx;
    type Transaction = reth_arbitrum_primitives::ArbTransactionSigned;
    type Receipt = reth_arbitrum_primitives::ArbReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <Self::EvmFactory as reth_evm::EvmFactory>::Evm<&'a mut revm::database::State<DB>, I>,
        ctx: ArbBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a + core::fmt::Debug,
        I: revm::inspector::Inspector<<Self::EvmFactory as reth_evm::EvmFactory>::Context<&'a mut revm::database::State<DB>>> + 'a,
        <DB as revm::Database>::Error: Send + Sync,
    {
        let eth_ctx: EthBlockExecutionCtx<'a> = EthBlockExecutionCtx {
            parent_hash: ctx.parent_hash,
            parent_beacon_block_root: ctx.parent_beacon_block_root,
            ommers: Default::default(),
            withdrawals: None,
        };
        ArbBlockExecutor {
            inner: EthBlockExecutor::new(
                evm,
                eth_ctx,
                alloy_evm::eth::spec::EthSpec::mainnet(),
                &self.receipt_builder,
            ),
            predeploys: self.predeploys.clone(),
            hooks: Default::default(),
            tx_state: Default::default(),
            _phantom: core::marker::PhantomData::<CS>,
        }
    }
}
