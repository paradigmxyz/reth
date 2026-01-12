#![allow(unused)]
extern crate alloc;

use alloc::{boxed::Box, sync::Arc};
use core::marker::PhantomData;
use std::sync::Mutex;
use alloy_consensus::Transaction;
use alloy_evm::{eth::EthEvmContext, precompiles::PrecompilesMap};
use alloy_primitives::{Address, B256, U256};
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
use crate::arbsys_precompile::{take_arbsys_state, clear_arbsys_state};
use crate::storage::{apply_arbsys_merkle_state, burn_arbsys_balance};

// Arbitrum-specific EthSpec that has Paris (The Merge) active from block 0.
// This is critical because Arbitrum is post-merge from genesis and should NOT give block rewards.
// Using EthSpec::mainnet() would cause 5 ETH to be given to the coinbase on each block!
use alloy_hardforks::{EthereumChainHardforks, EthereumHardfork, EthereumHardforks, ForkCondition};
// Import Arbitrum Sepolia hardfork timestamps
use alloy_hardforks::arbitrum::{
    ARBITRUM_SEPOLIA_SHANGHAI_TIMESTAMP,
    ARBITRUM_SEPOLIA_CANCUN_TIMESTAMP,
    ARBITRUM_SEPOLIA_PRAGUE_TIMESTAMP,
};

/// Arbitrum EthSpec with correct hardfork timings for Arbitrum Sepolia.
/// - Paris (and all prior hardforks) active from block 0 - prevents block rewards
/// - Shanghai at timestamp 1706634000 (block 10653737)
/// - Cancun at timestamp 1709229600 (block 18683405)
#[derive(Debug, Clone)]
pub struct ArbEthSpec {
    hardforks: EthereumChainHardforks,
}

impl Default for ArbEthSpec {
    fn default() -> Self {
        Self::arbitrum_sepolia()
    }
}

impl ArbEthSpec {
    /// Creates an ArbEthSpec for Arbitrum Sepolia with correct hardfork timings
    pub fn arbitrum_sepolia() -> Self {
        // Create hardforks with Paris at block 0 (critical to prevent block rewards!)
        // Shanghai, Cancun, Prague at their respective timestamps
        let hardforks = EthereumChainHardforks::new([
            (EthereumHardfork::Frontier, ForkCondition::Block(0)),
            (EthereumHardfork::Homestead, ForkCondition::Block(0)),
            (EthereumHardfork::Dao, ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine, ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon, ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium, ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople, ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg, ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul, ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier, ForkCondition::Block(0)),
            (EthereumHardfork::Berlin, ForkCondition::Block(0)),
            (EthereumHardfork::London, ForkCondition::Block(0)),
            (EthereumHardfork::ArrowGlacier, ForkCondition::Block(0)),
            (EthereumHardfork::GrayGlacier, ForkCondition::Block(0)),
            // Arbitrum: Paris at block 0 disables block rewards
            (EthereumHardfork::Paris, ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: Some(0),
                total_difficulty: alloy_primitives::U256::ZERO,
            }),
            // Use correct Arbitrum Sepolia timestamps for post-merge hardforks
            (EthereumHardfork::Shanghai, ForkCondition::Timestamp(ARBITRUM_SEPOLIA_SHANGHAI_TIMESTAMP)),
            (EthereumHardfork::Cancun, ForkCondition::Timestamp(ARBITRUM_SEPOLIA_CANCUN_TIMESTAMP)),
            (EthereumHardfork::Prague, ForkCondition::Timestamp(ARBITRUM_SEPOLIA_PRAGUE_TIMESTAMP)),
        ]);
        Self { hardforks }
    }
}

impl EthereumHardforks for ArbEthSpec {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        self.hardforks.ethereum_fork_activation(fork)
    }
}

impl alloy_evm::eth::spec::EthExecutorSpec for ArbEthSpec {
    fn deposit_contract_address(&self) -> Option<Address> {
        // Arbitrum doesn't have a deposit contract
        None
    }
}

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
    /// Chain ID for scheduled transactions
    pub chain_id: u64,
    /// Block timestamp for scheduled transactions
    pub block_timestamp: u64,
    /// Base fee for scheduled transactions
    pub basefee: U256,
}

pub struct ArbBlockExecutor<'a, Evm, CS, RB: alloy_evm::eth::receipt_builder::ReceiptBuilder> {
    inner: EthBlockExecutor<'a, Evm, ArbEthSpec, &'a RB>,
    predeploys: Arc<Mutex<PredeployRegistry>>,
    hooks: DefaultArbOsHooks,
    tx_state: ArbTxProcessorState,
    cumulative_gas_used: u64,
    exec_ctx: ArbBlockExecutionCtx,
    /// Deferred nonce restorations to apply at block end (address, target_nonce)
    /// For Internal/Retry txs that should NOT increment sender nonce
    pending_nonce_restorations: Vec<(Address, u64)>,
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
        // Clear zombie accounts from previous block.
        // Zombie tracking is per-block, not per-tx, so we clear at block start.
        reth_trie_common::zombie_sink::clear();
        
        // NOTE: We do NOT clear the shadow accumulator here because instrumentation showed
        // that apply_pre_execution_changes is called very frequently (per-tx or per-pass),
        // not just once per block. The shadow auto-resets when the block number changes
        // in update_shadow_accumulator, which is the correct behavior.
        
        // Also clear the deleted_empty_accounts set for this block
        self.tx_state.deleted_empty_accounts.clear();
        
        {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state_db: &mut revm::database::State<_> = *db_ref;
            match crate::arbosstate::ArbosState::open(state_db as *mut _) {
                Ok(arbos_state) => {
                // Load brotli compression level
                if let Ok(level) = arbos_state.get_brotli_compression_level() {
                    self.tx_state.brotli_compression_level = level as u32;
                } else {
                    self.tx_state.brotli_compression_level = 0;
                }

                // Arbitrum: load network_fee_account from ArbOS state
                match arbos_state.get_network_fee_account() {
                    Ok(network_fee) => {
                        self.tx_state.network_fee_account = network_fee;
                    },
                    Err(_) => {
                        tracing::error!(target: "arb::evm::build", "Failed to load network_fee_account from ArbOS");
                    }
                }

                // Load infra fee account
                if let Ok(infra_fee) = arbos_state.get_infra_fee_account() {
                    self.tx_state.infra_fee_account = infra_fee;
                }

                // Load ArbOS version (direct field access)
                self.tx_state.arbos_version = arbos_state.arbos_version;

                // Load min base fee for infra fee calculations
                if let Ok(min_fee) = arbos_state.l2_pricing_state.get_min_base_fee_wei() {
                    self.tx_state.min_base_fee = min_fee;
                }
                },
                Err(e) => {
                    tracing::error!(target: "arb::evm::build", "ArbosState::open failed: {}", e);
                    self.tx_state.brotli_compression_level = 0;
                }
            }
        }
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
                Deposit | Internal | Retry => false,
                _ => true,
            }
        };
        let is_internal = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Internal);
        let is_deposit = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Deposit);
        let is_retry = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Retry);
        let is_submit_retryable = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::SubmitRetryable);
        let is_contract = matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Contract);

        // Arbitrum: skip EIP-3607 for internal txs (precompiles have code but must send txs)
        let should_skip_eip3607 = is_internal || is_deposit || is_retry || is_submit_retryable || is_contract;
        if should_skip_eip3607 {
            self.evm_mut().cfg_mut().disable_eip3607 = true;
        } else {
            self.evm_mut().cfg_mut().disable_eip3607 = false;
        }

        // Arbitrum: capture balance for restoration (Internal/Deposit/SubmitRetryable only, NOT Retry)
        let needs_balance_restoration = is_internal || is_deposit || is_submit_retryable;
        let pre_start_tx_sender_balance = if needs_balance_restoration {
            let evm_temp = self.inner.evm_mut();
            let sender_info = evm_temp.db_mut().basic(sender).ok().flatten();
            Some(sender_info.map(|a| a.balance).unwrap_or_default())
        } else {
            None
        };

        // Arbitrum: no pre-credit for special tx types (start_tx hook handles minting)
        let needs_precredit = false;

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
        
        let (ticket_id, refund_to, gas_fee_cap_opt, max_refund, submission_fee_refund, deposit_value, retry_value, retry_to, retry_data, beneficiary, max_submission_fee, fee_refund_addr, l1_base_fee_opt) = match &**tx.tx() {
            reth_arbitrum_primitives::ArbTypedTransaction::Retry(retry_tx) => {
                (Some(retry_tx.ticket_id), Some(retry_tx.refund_to), Some(retry_tx.gas_fee_cap), Some(retry_tx.max_refund), Some(retry_tx.submission_fee_refund), None, None, None, None, None, None, None, None)
            },
            reth_arbitrum_primitives::ArbTypedTransaction::SubmitRetryable(submit_tx) => {
                (None, None, Some(submit_tx.gas_fee_cap), None, None, Some(submit_tx.deposit_value), Some(submit_tx.retry_value), submit_tx.retry_to, Some(submit_tx.retry_data.to_vec()), Some(submit_tx.beneficiary), Some(submit_tx.max_submission_fee), Some(submit_tx.fee_refund_addr), Some(submit_tx.l1_base_fee))
            },
            _ => (None, None, None, None, None, None, None, None, None, None, None, None, None),
        };
        
        let block_number = alloy_evm::Evm::block(self.evm()).number.try_into().unwrap_or(0);
        let parent_hash = self.exec_ctx.parent_hash;
        
        let start_ctx = ArbStartTxContext {
            sender,
            nonce,
            l1_base_fee: l1_base_fee_opt.unwrap_or(block_basefee),
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
            retry_data: retry_data.clone(),
            beneficiary,
            max_submission_fee,
            fee_refund_addr,
            block_timestamp,
            data: Some(tx.tx().input().to_vec()),
            block_number,
            parent_hash: Some(parent_hash),
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

        let hook_gas_override = if start_hook_result.end_tx_now {
            if let Some(err_msg) = start_hook_result.error {
                return Err(BlockExecutionError::msg(err_msg));
            }
            
            Some(start_hook_result.gas_used)
        } else {
            None
        };

        let tx_type = tx.tx().tx_type();
        use reth_arbitrum_primitives::ArbTxType;

        let tx_bytes = {
            use alloy_eips::eip2718::Encodable2718;
            let mut buf = Vec::new();
            tx.tx().encode_2718(&mut buf);
            buf
        };
        
        let calldata_vec = tx.tx().input().to_vec();
        
        let poster = if block_coinbase == crate::l1_pricing::BATCH_POSTER_ADDRESS {
            crate::l1_pricing::BATCH_POSTER_ADDRESS
        } else {
            Address::ZERO
        };
        
        let gas_ctx = ArbGasChargingContext {
            intrinsic_gas: 21_000,
            calldata: calldata_vec,
            tx_bytes,
            basefee: block_basefee,
            is_executed_on_chain: true,
            skip_l1_charging: false,
            poster,
            gas_remaining: tx.tx().gas_limit(),
            is_ethcall: false,
            tx_type: tx_type_u8,
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

        // NOTE: Deposit transactions are fully handled in start_tx hook (execute.rs)
        // which mints balance to sender and transfers to recipient.
        // Do NOT call execute_deposit here as it would duplicate the deposit.

        let mut maybe_predeploy_result: Option<(revm::context::result::ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>, u64)> = None;
        // Skip predeploy dispatch for transactions that are fully handled in start_tx
        // (internal, deposits, submit retryables) to preserve logs emitted in start_tx
        if !start_hook_result.end_tx_now {
            if let Some(call_to) = to_addr {
                let evm = self.inner.evm();
                let block = alloy_evm::Evm::block(evm);
                let ctx = PredeployCallContext {
                    block_number: u64::try_from(block.number).unwrap_or(0),
                    l1_block_number: self.tx_state.cached_l1_block_number.unwrap_or(0),
                    block_hashes: alloc::vec::Vec::new(),
                    chain_id: alloy_primitives::U256::from(self.inner.evm().chain_id()),
                    os_version: 0,
                    time: u64::try_from(block.timestamp).unwrap_or(0),
                    origin: sender,
                    caller: sender,
                    depth: 0,
                    basefee: block_basefee,
                };
                // Only clear logs for transactions that are NOT handled in start_tx
                // start_tx emissions must be preserved
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
                    let calldata_bytes = tx.tx().input().clone();
                    let _ = reg.dispatch_with_emitter(&ctx, call_to, &calldata_bytes, gas_limit, alloy_primitives::U256::from(tx.tx().value()), &mut retryables, &mut emitter);
                }
            }
        }

        let current_nonce = {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            let nonce = match state.basic(sender) {
                Ok(info_opt) => info_opt.map(|i| i.nonce).unwrap_or_default(),
                Err(_) => 0,
            };

            nonce
        };


        let mut tx_env = tx.to_tx_env();

        // Arbitrum: gas_price=basefee for all txs (DropTip=true for ArbOS!=v9)
        reth_evm::TransactionEnv::set_gas_price(&mut tx_env, block_basefee.to::<u128>());

        if is_deposit {
            // For Deposit transactions, set nonce
            reth_evm::TransactionEnv::set_nonce(&mut tx_env, current_nonce);
        }
        // Arbitrum: nonce restoration for special tx types
        // Internal->0, Retry->current, Deposit->current, SubmitRetryable->no restoration
        let pre_exec_nonce = if is_internal {
            Some(0)
        } else if is_retry || is_deposit {
            Some(current_nonce)
        } else {
            None
        };

        if needs_precredit {
            // Pre-credit sender with gas fees for Internal/Retry transactions
            // Note: disable_nonce_check only skips validation, nonce will still be incremented
            // We restore nonce IMMEDIATELY after execution (tracked in pre_exec_nonce)

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
            tracing::debug!(
                target: "arb::evm::build",
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






        let evm = self.inner.evm_mut();
        let prev_disable_balance = evm.cfg_mut().disable_balance_check;
        let prev_disable_nonce = evm.cfg_mut().disable_nonce_check;
        // In trust sequencer mode (skip_state_root_validation=true), we disable balance checks for ALL
        // transaction types because our computed state may diverge from the real state.
        // The sequencer has already validated all transactions before including them.
        //
        // For internal/deposit/submit_retryable: always disable (hooks handle balance)
        // For user transactions: disable because we trust the sequencer
        let new_disable_balance = true; // Trust sequencer mode: always disable balance check
        evm.cfg_mut().disable_balance_check = new_disable_balance;
        // In trust sequencer mode (skip_state_root_validation=true), we also disable nonce checks
        // for ALL transaction types because our computed state may diverge from the real state.
        // The sequencer has already validated all nonces before including them.
        //
        // Note: disable_nonce_check only disables VALIDATION, not increment!
        // The nonce will still be incremented after execution.
        let new_disable_nonce = true; // Trust sequencer mode: always disable nonce check
        evm.cfg_mut().disable_nonce_check = new_disable_nonce;

        // Arbitrum: validate balance for user txs (delayed inbox txs bypass sequencer)
        // balance validation for ALL user transactions, we correctly handle both:
        // - Sequencer-validated txs: will always have sufficient balance (no rejection)
        // - Delayed inbox txs: may have insufficient balance (correctly rejected)
        let is_user_tx = !is_internal && !is_deposit && !is_submit_retryable && !is_retry;
        if is_user_tx {
            let sender_balance_result = self.inner.evm_mut().db_mut().basic(sender);
            let sender_balance = sender_balance_result.ok().flatten().map(|a| a.balance).unwrap_or_default();
            let gas_cost = alloy_primitives::U256::from(gas_limit) * upfront_gas_price;
            let tx_value = tx.tx().value();
            let total_cost = gas_cost.saturating_add(tx_value);

            if sender_balance < total_cost {
                return Err(BlockExecutionError::msg(format!(
                    "insufficient funds for gas * price + value: address {} have {} want {}",
                    sender, sender_balance, total_cost
                )));
            }
        }

        let wrapped = WithTxEnv { tx_env, tx };

        // Arbitrum: capture gas_used from EVM for end_tx hook
        let captured_gas_used = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let captured_gas_used_clone = captured_gas_used.clone();

        // Execute transaction through EVM
        // For early-terminated transactions (end_tx_now=true), the state was already
        // modified in start_tx hook, but we still execute through EVM to build receipt.
        // Note: All predeploys have 0xFE code, but early-terminated txs bypass EVM execution.
        // Capture poster_gas before the closure to use inside
        let poster_gas_for_closure = self.tx_state.poster_gas;

        let result = self.inner.execute_transaction_with_commit_condition(wrapped, |exec_result| {
            let evm_gas = exec_result.gas_used();
            let actual_gas = hook_gas_override.unwrap_or(evm_gas);

            // Arbitrum: add poster_gas (L1 data cost) to gas_used
            let total_gas_with_l1 = actual_gas.saturating_add(poster_gas_for_closure);

            // Internal transactions (type 0x6a) don't contribute to cumulative gas
            let gas_to_add = if is_internal { 0u64 } else { total_gas_with_l1 };
            let new_cumulative = self.cumulative_gas_used + gas_to_add;

            captured_gas_used_clone.store(actual_gas, std::sync::atomic::Ordering::SeqCst);

            // For internal txs, store 0 as the gas used in receipt
            // For other txs, include L1 data cost in gas_for_receipt
            let gas_for_receipt = if is_internal { 0u64 } else { total_gas_with_l1 };
            crate::set_early_tx_gas(tx_hash, gas_for_receipt, new_cumulative);

            f(exec_result)
        });

        let actual_gas_used = captured_gas_used.load(std::sync::atomic::Ordering::SeqCst);
        let gas_left_for_end_tx = gas_limit.saturating_sub(actual_gas_used);

        // Apply deferred ArbSys state changes using the proper transition mechanism.
        // This is the key fix: the precompile stores state changes in a thread-local sink,
        // and we apply them here using storage.rs which uses state.apply_transition().
        // This ensures the state changes survive merge_transitions() and end up in bundle_state.
        //
        // Arbitrum: only apply deferred state changes on committing pass
        let is_committing_pass = matches!(result, Ok(Some(_)));
        
        if let Some(arbsys_state) = take_arbsys_state() {
            if is_committing_pass {
                // Apply deferred ArbSys state changes for this committing pass.
                // IMPORTANT: We no longer use global dedupe or shadow accumulator because they cause
                // cross-context contamination when the same transaction is executed in multiple contexts.
                // Each execution context computes its own leaf_num from the database state, and the
                // canonical context's state changes will be the ones that persist.
                tracing::debug!(
                    target: "arb::arbsys_deferred",
                    tx_hash = ?tx_hash,
                    new_size = arbsys_state.new_size,
                    partials_count = arbsys_state.partials.len(),
                    send_hash = ?arbsys_state.send_hash,
                    leaf_num = arbsys_state.leaf_num,
                    value_to_burn = %arbsys_state.value_to_burn,
                    "Applying deferred ArbSys Merkle state changes (committing pass)"
                );
                
                // Get mutable access to the EVM state
                let evm = self.inner.evm_mut();
                let (db_ref, _, _) = evm.components_mut();
                let state: &mut revm::database::State<D> = *db_ref;
                
                // Apply the Merkle accumulator state changes using storage.rs
                apply_arbsys_merkle_state(state, arbsys_state.new_size, arbsys_state.partials);
                
                // Handle value burning: subtract the burned value from ArbSys account balance
                // The EVM has already transferred the value from caller to ArbSys (0x64),
                // so we need to burn it (subtract from ArbSys balance)
                // Use burn_arbsys_balance which uses the proper transition mechanism
                // to ensure the balance change survives merge_transitions()
                if arbsys_state.value_to_burn > U256::ZERO {
                    burn_arbsys_balance(state, arbsys_state.value_to_burn);
                }
                
                tracing::debug!(
                    target: "arb::arbsys_deferred",
                    tx_hash = ?tx_hash,
                    block_number = arbsys_state.block_number,
                    new_size = arbsys_state.new_size,
                    "Successfully applied deferred ArbSys state changes"
                );
            } else {
                tracing::debug!(
                    target: "arb::arbsys_deferred",
                    tx_hash = ?tx_hash,
                    new_size = arbsys_state.new_size,
                    "Skipping deferred ArbSys state changes (non-committing pass)"
                );
            }
        }

        if let Ok(Some(evm_gas)) = result {
            // Internal transactions don't contribute to cumulative gas
            let actual_gas = hook_gas_override.unwrap_or(evm_gas);

            // Arbitrum: add poster_gas (L1 data cost) to gas_used
            let poster_gas = self.tx_state.poster_gas;
            let total_gas_with_l1 = actual_gas.saturating_add(poster_gas);

            let final_gas_to_add = if is_internal { 0u64 } else { total_gas_with_l1 };
            self.cumulative_gas_used += final_gas_to_add;
        }

        // Arbitrum: defer nonce restoration for special txs until finish()
        if let Some(pre_nonce) = pre_exec_nonce {
            // Access the EVM state to check current nonce after execution
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            let current_nonce_after_exec = if let Some(cached_acc) = state.cache.accounts.get(&sender) {
                if let Some(ref account) = cached_acc.account {
                    account.info.nonce
                } else {
                    0  // Should not happen
                }
            } else {
                0  // Should not happen
            };

            self.pending_nonce_restorations.push((sender, pre_nonce));
        }

        // Arbitrum: restore balance for special txs (NOT Retry - end_tx handles that)
        if let Some(expected_balance) = pre_start_tx_sender_balance {
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            // Get current balance after execution
            let current_balance_after_exec = if let Some(cached_acc) = state.cache.accounts.get(&sender) {
                if let Some(ref account) = cached_acc.account {
                    account.info.balance
                } else {
                    alloy_primitives::U256::ZERO
                }
            } else {
                alloy_primitives::U256::ZERO
            };

            if current_balance_after_exec != expected_balance {
                // Update the cache to restore the correct balance
                if let Some(cached_acc) = state.cache.accounts.get_mut(&sender) {
                    if let Some(ref mut account) = cached_acc.account {
                        account.info.balance = expected_balance;
                    }
                }

                // Arbitrum: also update transition_state for bundle_state creation
                if let Some(ref mut transition_state) = state.transition_state {
                    if let Some(transition_acc) = transition_state.transitions.get_mut(&sender) {
                        if let Some(ref mut info) = transition_acc.info {
                            info.balance = expected_balance;
                        }
                    }
                }
            }
        }

        let evm = self.inner.evm_mut();
        evm.cfg_mut().disable_balance_check = prev_disable_balance;
        evm.cfg_mut().disable_nonce_check = prev_disable_nonce;

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
        
        // Arbitrum: skip end_tx for early-terminated txs (end_tx_now=true)
        if !start_hook_result.end_tx_now {
            let end_ctx = ArbEndTxContext {
                success: result.is_ok(),
                gas_left: gas_left_for_end_tx,
                gas_limit,
                basefee: block_basefee,
                // Use paid_gas_price for gas pool update check (matches Go's msg.GasPrice.Sign() > 0)
                gas_price: paid_gas_price,
                tx_type: tx_type_u8,
                block_timestamp,
                sender,
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
        } else {
            tracing::debug!(
                target: "arb::evm::build",
                tx_type = ?tx_type,
                tx_hash = ?tx_hash,
                "Skipping end_tx hook for early-terminated transaction (end_tx_now=true)"
            );
        }

        // ZOMBIE TRACKING: MUST happen BEFORE check_and_push_scheduled_txes!
        // This is the Rust equivalent of Go nitro's stateObjectsDestruct population in Finalise().
        // In Go nitro, Finalise() is called at the end of each tx via IntermediateRoot, which populates
        // stateObjectsDestruct BEFORE the next transaction starts. The retry tx's start_tx hook calls
        // CreateZombieIfDeleted, which checks stateObjectsDestruct. So we must track deletions BEFORE
        // scheduling the retry tx.
        //
        // ZOMBIE TRACKING: After each transaction, track accounts that were deleted during EIP-161 cleanup.
        // This is the Rust equivalent of Go nitro's stateObjectsDestruct population in Finalise().
        // We check the transition_state for accounts that are empty and would be deleted.
        {
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            
            // Iterate over all accounts in the transition state
            if let Some(ref transition_state) = state.transition_state {
                for (addr, transition_acc) in &transition_state.transitions {
                    // Check if this account is empty and would be deleted
                    let is_empty = transition_acc.info.as_ref()
                        .map(|info| info.is_empty())
                        .unwrap_or(false);
                    
                    // Log the status for debugging
                    tracing::trace!(
                        target: "arbitrum::zombie::status",
                        ?addr,
                        status = ?transition_acc.status,
                        is_empty = is_empty,
                        has_info = transition_acc.info.is_some(),
                        previous_status = ?transition_acc.previous_status,
                        has_previous_info = transition_acc.previous_info.is_some(),
                        tx_hash = ?tx_hash,
                        "Post-tx account status check"
                    );
                    
                    // Track accounts that are empty and would be deleted during EIP-161 cleanup.
                    // In Go nitro, this happens in Finalise() when:
                    // - deleteEmptyObjects is true (always true post-Spurious Dragon)
                    // - obj.empty() is true (nonce=0, balance=0, no code)
                    // - !isZombie (not already a zombie)
                    //
                    // We use the account status to detect deletions:
                    // - Destroyed, DestroyedChanged, DestroyedAgain = self-destruct
                    // - LoadedEmptyEIP161 = EIP-161 empty account cleanup
                    // - InMemoryChange with empty info = newly created empty account that will be deleted
                    use revm::database::AccountStatus;
                    let is_deleted = match transition_acc.status {
                        AccountStatus::Destroyed | 
                        AccountStatus::DestroyedChanged | 
                        AccountStatus::DestroyedAgain => true,
                        AccountStatus::LoadedEmptyEIP161 => true,
                        // For InMemoryChange, check if the account is empty
                        AccountStatus::InMemoryChange | AccountStatus::Changed => is_empty,
                        _ => false,
                    };
                    
                    if is_deleted && !self.tx_state.zombie_accounts.contains(addr) {
                        // Add to deleted_empty_accounts for CreateZombieIfDeleted to check
                        self.tx_state.deleted_empty_accounts.insert(*addr);
                        tracing::debug!(
                            target: "arbitrum::zombie",
                            ?addr,
                            status = ?transition_acc.status,
                            "Tracked account as deleted (EIP-161 cleanup)"
                        );
                    }
                }
            }
            
            // FINALISE-LIKE STEP: Remove deleted empty accounts from the cache.
            // This matches Go geth's Finalise() behavior where deleted accounts are removed
            // from the stateObjects map, so that getStateObject() returns nil for them.
            // This is critical for CreateZombieIfDeleted to work correctly - it checks
            // if getStateObject(addr) == nil && wasDeletedEarlierInBlock.
            // 
            // IMPORTANT: Do NOT remove zombie accounts from the cache! Zombie accounts are
            // accounts that were deleted earlier in the block but then resurrected via
            // CreateZombieIfDeleted. They must remain in the cache so they persist in the trie.
            for addr in &self.tx_state.deleted_empty_accounts {
                // Skip zombie accounts - they should remain in the cache
                if self.tx_state.zombie_accounts.contains(addr) {
                    tracing::debug!(
                        target: "arbitrum::zombie",
                        ?addr,
                        "Finalise: skipping zombie account (not removing from cache)"
                    );
                    continue;
                }
                if let Some(cached) = state.cache.accounts.get_mut(addr) {
                    // Mark the account as non-existent by setting account to None
                    // This matches geth's behavior where deleted accounts are removed from stateObjects
                    cached.account = None;
                    tracing::debug!(
                        target: "arbitrum::zombie",
                        ?addr,
                        "Finalise: removed deleted empty account from cache"
                    );
                }
            }
        }

        // Check for and push any scheduled transactions (retry txs) to the sink
        // This matches Go behavior where ScheduledTxes are collected after each tx
        // IMPORTANT: This MUST happen AFTER deletion tracking above, so that when the retry tx
        // starts and calls CreateZombieIfDeleted, the deleted_empty_accounts set is populated.
        self.check_and_push_scheduled_txes();

        result
    }

    fn finish(mut self) -> Result<(Self::Evm, RethBlockExecutionResult<reth_arbitrum_primitives::ArbReceipt>), BlockExecutionError> {
        // Arbitrum: apply nonce restorations to both cache and transition_state
        if !self.pending_nonce_restorations.is_empty() {
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            for (sender, target_nonce) in &self.pending_nonce_restorations {
                // Update cache
                if let Some(cached_acc) = state.cache.accounts.get_mut(sender) {
                    if let Some(ref mut account) = cached_acc.account {
                        account.info.nonce = *target_nonce;
                    }
                }

                // Arbitrum: also update transition_state for bundle_state creation
                if let Some(ref mut transition_state) = state.transition_state {
                    if let Some(transition_acc) = transition_state.transitions.get_mut(sender) {
                        if let Some(ref mut info) = transition_acc.info {
                            info.nonce = *target_nonce;
                        }
                    }
                }
            }
        }

        let (mut evm, mut result) = self.inner.finish()?;

        // Arbitrum: correct gasUsed from cumulative gas in last receipt
        if let Some(last_receipt) = result.receipts.last() {
            use alloy_consensus::TxReceipt;
            result.gas_used = last_receipt.cumulative_gas_used();
        }

        Ok((evm, result))
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

impl<'a, E, CS, RB, D> ArbBlockExecutor<'a, E, CS, RB>
where
    RB: alloy_evm::eth::receipt_builder::ReceiptBuilder<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt>,
    D: RevmDatabase + core::fmt::Debug + 'a,
    <D as RevmDatabase>::Error: Send + Sync + 'static,
    E: reth_evm::Evm<DB = &'a mut revm::database::State<D>> + crate::arb_evm::ArbEvmExt,
    E::Tx: Clone + alloy_evm::tx::FromRecoveredTx<reth_arbitrum_primitives::ArbTransactionSigned> + alloy_evm::tx::FromTxWithEncoded<reth_arbitrum_primitives::ArbTransactionSigned>,
    for<'b> alloy_evm::eth::EthBlockExecutor<'b, E, alloy_evm::eth::spec::EthSpec, &'b RB>: alloy_evm::block::BlockExecutor<Transaction = reth_arbitrum_primitives::ArbTransactionSigned, Receipt = reth_arbitrum_primitives::ArbReceipt, Evm = E>,
    <E as alloy_evm::Evm>::Tx: reth_evm::TransactionEnv,
{
    /// Check for and push scheduled transactions (retry transactions) to the sink after a transaction execution.
    /// This should be called after each transaction execution to collect any scheduled redeems.
    /// The scheduled transactions can be retrieved from the sink using `scheduled_tx_sink::take()`.
    fn check_and_push_scheduled_txes(&mut self) {
        // Get the logs from the log sink (these are the logs from the last transaction)
        let logs = crate::log_sink::take();
        
        if logs.is_empty() {
            return;
        }
        
        // Get block context for scheduled_txes
        let chain_id = self.exec_ctx.chain_id;
        let block_timestamp = self.exec_ctx.block_timestamp;
        let basefee = self.exec_ctx.basefee;
        
        tracing::debug!(
            target: "arb-scheduled",
            logs_count = logs.len(),
            chain_id = chain_id,
            block_timestamp = block_timestamp,
            basefee = %basefee,
            "Checking for scheduled transactions"
        );
        
        // Call scheduled_txes to get any scheduled retry transactions
        let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
        let state_db: &mut revm::database::State<D> = *db_ref;
        
        let scheduled = self.hooks.scheduled_txes(
            state_db,
            &self.tx_state,
            &logs,
            chain_id,
            block_timestamp,
            basefee,
        );
        
        if !scheduled.is_empty() {
            tracing::debug!(
                target: "arb-scheduled",
                scheduled_count = scheduled.len(),
                "Found scheduled transactions to execute"
            );
            // Push scheduled transactions to the sink for retrieval by node.rs
            for tx in scheduled {
                crate::scheduled_tx_sink::push(tx);
            }
        }
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
                ArbEthSpec::arbitrum_sepolia(),
                &self.receipt_builder,
            ),
            predeploys: self.predeploys.clone(),
            hooks: Default::default(),
            tx_state: Default::default(),
            cumulative_gas_used: 0,
            exec_ctx: ctx,
            pending_nonce_restorations: Vec::new(),
            _phantom: core::marker::PhantomData::<CS>,
        }
    }
}
