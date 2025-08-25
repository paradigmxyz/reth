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
use revm::{
    context::TxEnv,
    inspector::Inspector,
    interpreter::interpreter::EthInterpreter
};
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput, WithTxEnv, ExecutorTx};
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::BlockExecutionResult as RethBlockExecutionResult;
use reth_evm::{OnStateHook, TransactionEnv};
use alloy_evm::Database;
use revm::Database as RevmDatabase;
use alloy_evm::block::{BlockExecutorFactory, BlockExecutorFor, CommitChanges, ExecutableTx, BlockExecutor as AlloyBlockExecutor};
use alloy_evm::eth::{EthBlockExecutionCtx, EthBlockExecutor};
use alloy_evm::ToTxEnv;
use revm::context::result::ExecutionResult;

use crate::predeploys::PredeployRegistry;
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
}

pub struct ArbBlockExecutor<'a, Evm, CS, RB: alloy_evm::eth::receipt_builder::ReceiptBuilder> {
    inner: EthBlockExecutor<'a, Evm, alloy_evm::eth::spec::EthSpec, &'a RB>,
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
    E: reth_evm::Evm<DB = &'a mut revm::database::State<D>>,
    E::Tx: Clone + alloy_evm::tx::FromRecoveredTx<reth_arbitrum_primitives::ArbTransactionSigned> + alloy_evm::tx::FromTxWithEncoded<reth_arbitrum_primitives::ArbTransactionSigned>,
    for<'b> alloy_evm::eth::EthBlockExecutor<'b, E, alloy_evm::eth::spec::EthSpec, &'b RB>: alloy_evm::block::BlockExecutor<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt, Evm = E>,
    <E as alloy_evm::Evm>::Tx: reth_evm::TransactionEnv,
{
    type Transaction = reth_arbitrum_primitives::ArbTransactionSigned;
    type Receipt = reth_arbitrum_primitives::ArbReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
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
        let needs_precredit = {
            use reth_arbitrum_primitives::ArbTxType::*;
            match tx.tx().tx_type() {
                Internal => false,
                Deposit => true,
                _ => true,
            }
        };

        let paid_gas_price = {
            use reth_arbitrum_primitives::ArbTxType::*;
            match tx.tx().tx_type() {
                Legacy => block_basefee,
                Deposit | Internal => block_basefee,
                _ => alloy_primitives::U256::from(tx.tx().max_fee_per_gas()),
            }
        };
        let upfront_gas_price = alloy_primitives::U256::from(tx.tx().max_fee_per_gas());

        let start_ctx = ArbStartTxContext {
            sender,
            nonce,
            l1_base_fee: block_basefee,
            calldata_len,
            coinbase: block_coinbase,
            executed_on_chain: true,
            is_eth_call: false,
        };
        {
            let mut state = core::mem::take(&mut self.tx_state);
            {
                let evm = self.inner.evm_mut();
                self.hooks.start_tx::<E>(evm, &mut state, &start_ctx);
            }
            self.tx_state = state;
        }

        let gas_ctx = ArbGasChargingContext {
            intrinsic_gas: 21_000,
            calldata: calldata.to_vec(),
            basefee: block_basefee,
            is_executed_on_chain: true,
            skip_l1_charging: false,
        };
        {
            let mut state = core::mem::take(&mut self.tx_state);
            let res = {
                let evm = self.inner.evm_mut();
                self.hooks.gas_charging::<E>(evm, &mut state, &gas_ctx)
            };
            self.tx_state = state;
            let _ = res;
        }

        let mut used_pre_nonce = None;

        let current_nonce = {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            match state.basic(sender) {
                Ok(info_opt) => info_opt.map(|i| i.nonce).unwrap_or_default(),
                Err(_) => 0,
            }
        };

        let mut tx_env = tx.to_tx_env();
        if is_internal {
            reth_evm::TransactionEnv::set_gas_price(&mut tx_env, 0u128);
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

            use alloy_primitives::map::HashMap;
            use alloy_evm::block::state_changes::balance_increment_state;
            use revm::state::AccountInfo;
            use alloy_consensus::constants::KECCAK_EMPTY;
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            let mut increments = HashMap::default();
            increments.insert(sender, needed_fee.to::<u128>());
            let _ = balance_increment_state(&increments, state);

            let existing = match state.basic(sender) {
                Ok(Some(info)) => info,
                _ => AccountInfo { balance: alloy_primitives::U256::ZERO, nonce: current_nonce, code_hash: KECCAK_EMPTY, code: None },
            };
            let mut updated = existing;
            updated.balance = updated.balance.saturating_add(needed_fee);
            state.insert_account(sender, updated);

            let overlay_bal = state
                .bundle_state
                .state
                .get(&sender)
                .and_then(|acc| acc.info.as_ref().map(|i| i.balance))
                .unwrap_or_default();
            tracing::info!(target: "arb-reth::executor", sender_balance_after_precredit_overlay = %overlay_bal, "bundle overlay balance after pre-credit");
        }






        if let Some(pre_nonce) = used_pre_nonce {
            reth_evm::TransactionEnv::set_nonce(&mut tx_env, pre_nonce);
        }

        let wrapped = WithTxEnv { tx_env, tx };
        let result = self.inner.execute_transaction_with_commit_condition(wrapped, f);

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

        let res = result;

        let end_ctx = ArbEndTxContext {
            success: res.is_ok(),
            gas_left: 0,
            gas_limit,
            basefee: block_basefee,
        };
        {
            let mut state = core::mem::take(&mut self.tx_state);
            {
                let evm = self.inner.evm_mut();
                self.hooks.end_tx::<E>(evm, &mut state, &end_ctx);
            }
            self.tx_state = state;
        }

        res
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
            hooks: Default::default(),
            tx_state: Default::default(),
            _phantom: core::marker::PhantomData::<CS>,
        }
    }
}
