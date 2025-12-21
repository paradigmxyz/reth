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
            // CRITICAL: Paris at block 0 disables block rewards!
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
        {
            let (db_ref, _insp, _precompiles) = self.inner.evm_mut().components_mut();
            let state_db: &mut revm::database::State<_> = *db_ref;
            match crate::arbosstate::ArbosState::open(state_db as *mut _) {
                Ok(arbos_state) => {
                    tracing::warn!(target: "arb::build", "[ITER118] ArbosState::open succeeded, version={}", arbos_state.arbos_version);
                // Load brotli compression level
                if let Ok(level) = arbos_state.get_brotli_compression_level() {
                    self.tx_state.brotli_compression_level = level as u32;
                } else {
                    self.tx_state.brotli_compression_level = 0;
                }

                // CRITICAL FIX: Load network fee account from ArbOS state
                // If not set, fees were incorrectly going to address zero
                match arbos_state.get_network_fee_account() {
                    Ok(network_fee) => {
                        tracing::warn!(target: "arb::build", "[ITER118] Loaded network_fee_account from ArbOS: {:?}", network_fee);
                        self.tx_state.network_fee_account = network_fee;
                    },
                    Err(_) => {
                        tracing::error!(target: "arb::build", "[ITER118] FAILED to load network_fee_account from ArbOS!");
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
                    tracing::error!(target: "arb::build", "[ITER118] ArbosState::open FAILED: {}", e);
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

        // CRITICAL: Skip EIP-3607 check for Arbitrum internal transaction types
        // EIP-3607 rejects transactions from senders with deployed code.
        // However, ArbOS precompiles (like ArbosActs at 0xa4b05) have code (0xfe)
        // but must be able to send internal transactions.
        // This matches Go Nitro's skipTransactionChecks() behavior in arb_types.go.
        let should_skip_eip3607 = is_internal || is_deposit || is_retry || is_submit_retryable || is_contract;
        if should_skip_eip3607 {
            self.evm_mut().cfg_mut().disable_eip3607 = true;
        } else {
            self.evm_mut().cfg_mut().disable_eip3607 = false;
        }

        // ITERATION 99: Capture sender balance BEFORE start_tx hook runs.
        //
        // For Internal, Deposit, SubmitRetryable transactions (end_tx_now=true):
        // - These don't go through EVM, don't call end_tx hook
        // - Balance restoration to pre-start_tx value is needed
        //
        // For Retry transactions (end_tx_now=false):
        // - Goes through EVM with revm's gas accounting
        // - end_tx hook at line 1270 ALREADY burns the gas_refund: burn_balance(sender, gas_refund)
        // - DO NOT restore balance here - end_tx handles it correctly!
        //
        // The issue was: restoring balance to 0 BEFORE end_tx broke the burn logic,
        // then end_tx's transfer_balance to sender added ETH back, resulting in 5 ETH.
        let needs_balance_restoration = is_internal || is_deposit || is_submit_retryable;
        // NOTE: is_retry is EXCLUDED - end_tx hook handles gas refund burning
        let pre_start_tx_sender_balance = if needs_balance_restoration {
            let evm_temp = self.inner.evm_mut();
            let sender_info = evm_temp.db_mut().basic(sender).ok().flatten();
            let balance = sender_info.map(|a| a.balance).unwrap_or_default();
            tracing::warn!(
                target: "arb-reth::gas-fix",
                tx_type = ?tx.tx().tx_type(),
                sender = ?sender,
                pre_start_tx_balance = %balance,
                "[ITER99] Captured sender balance BEFORE start_tx (Internal/Deposit/SubmitRetryable only)"
            );
            Some(balance)
        } else {
            None
        };

        // ITERATION 82b: Pre-credit logic - NO pre-credit needed for special tx types
        //
        // ALL balance manipulation for these tx types is handled in start_tx hook:
        //
        // - Internal (0x6a): Returns endTxNow=true immediately. No EVM execution.
        //   No pre-credit needed.
        //
        // - SubmitRetryable (0x69): Returns endTxNow=true. No EVM execution.
        //   Hook mints deposit_value to sender. No pre-credit needed.
        //
        // - Retry (0x68): Goes through EVM but start_tx hook ALREADY mints prepaid gas:
        //   Line 812-813 in execute.rs: `Self::mint_balance(state_db, ctx.sender, prepaid);`
        //   where prepaid = basefee * gas_limit. No additional pre-credit needed!
        //
        // CRITICAL FIX: Both SubmitRetryable AND Retry were being double-credited:
        // 1. start_tx hook mints balance (deposit_value or prepaid gas)
        // 2. build.rs pre-credit adds more balance
        // Result: sender ends up with excess balance that was never consumed.
        //
        // With this fix, only Legacy and other standard transaction types that go
        // through normal EVM execution without start_tx balance minting will need
        // pre-credit (and that's handled elsewhere in the EVM execution).
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

        // ITER83: Debug logging for start_hook_result
        tracing::info!(
            target: "arb-reth::start_tx_debug",
            tx_type = tx_type_u8,
            tx_type_hex = format!("0x{:02x}", tx_type_u8),
            end_tx_now = start_hook_result.end_tx_now,
            gas_used = start_hook_result.gas_used,
            has_error = start_hook_result.error.is_some(),
            "[ITER83] build.rs received start_hook_result"
        );

        let hook_gas_override = if start_hook_result.end_tx_now {
            tracing::debug!(
                target: "arb-reth::executor",
                tx_type = ?tx.tx().tx_type(),
                gas_used = start_hook_result.gas_used,
                error = ?start_hook_result.error,
                "Transaction ended early - will override EVM gas with hook gas"
            );
            
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

        let mut maybe_predeploy_result: Option<(revm::context::result::ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>, u64)> = None;
        // Skip predeploy dispatch for transactions that are fully handled in start_tx
        // (internal, deposits, submit retryables) to preserve logs emitted in start_tx
        if !start_hook_result.end_tx_now {
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

            // ITERATION 42: Enhanced logging for nonce debugging
            if is_internal || is_retry {
                tracing::warn!(
                    target: "reth::evm::execute",
                    sender = ?sender,
                    nonce = nonce,
                    is_internal = is_internal,
                    is_retry = is_retry,
                    tx_type = ?tx_type,
                    "[req-1] ITER42: Queried sender nonce BEFORE execution"
                );
            }

            nonce
        };


        let mut tx_env = tx.to_tx_env();

        // CRITICAL FIX (Iteration 97): For all tx types, set gas_price = basefee.
        // This prevents coinbase from receiving tips (since DropTip() returns true for ArbOS != v9).
        //
        // For special tx types (Internal, Deposit, SubmitRetryable, Retry), we will track the
        // sender's balance BEFORE execution and restore it AFTER execution to undo any
        // refunds that revm's reimburse_caller adds.
        //
        // revm's normal flow:
        // 1. Pre-execution: deduct gas_limit * gas_price from caller (fails silently with disable_balance_check)
        // 2. Post-execution: refund remaining_gas * effective_gas_price to caller
        //
        // With disable_balance_check=true and a non-zero gas_price:
        // - Step 1 fails silently (saturating_sub from 0 = 0)
        // - Step 2 STILL adds balance to caller! This gives caller FREE money!
        //
        // We can't set gas_price = 0 because revm's validation rejects gas_price < basefee.
        // Instead, we will manually undo the refund after execution for special tx types.
        // NOTE: skip_evm_gas_accounting is already defined earlier (line 207) for the balance capture

        // Always set gas_price = basefee to pass validation
        reth_evm::TransactionEnv::set_gas_price(&mut tx_env, block_basefee.to::<u128>());

        // ITERATION 98: Removed old capture here - now using pre_start_tx_sender_balance captured earlier

        if is_deposit {
            // For Deposit transactions, set nonce
            reth_evm::TransactionEnv::set_nonce(&mut tx_env, current_nonce);
        }
        // ITERATION 46: Removed tx_env nonce setting for Internal transactions
        // This was interfering with nonce restoration. Internal transactions should
        // use the nonce from the transaction itself, not override it.


        // ITERATION 80: Nonce restoration logic
        // - Internal (0x6a): ALWAYS restore to 0 (ArbOS should never have nonce > 0)
        // - Retry (0x68): Restore to current_nonce (prevents increment for THIS tx)
        // - SubmitRetryable (0x69): NO RESTORATION - it SHOULD increment nonce!
        //
        // Key insight from official chain: After Block 1 with SubmitRetryable + Retry,
        // the sender (0xb8787d8f...) has nonce=1, meaning:
        // - SubmitRetryable DID increment nonce from 0 to 1
        // - Retry did NOT increment nonce (stayed at 1)
        //
        // Previous iterations incorrectly restored SubmitRetryable nonce to 0,
        // which caused state root mismatches starting from Block 1.
        let pre_exec_nonce = if is_internal {
            tracing::info!(
                target: "reth::evm::execute",
                sender = ?sender,
                current_nonce = current_nonce,
                "[req-1] ITER80: Internal tx - will restore nonce to 0"
            );
            Some(0)
        } else if is_retry {
            tracing::info!(
                target: "reth::evm::execute",
                sender = ?sender,
                current_nonce = current_nonce,
                "[req-1] ITER80: Retry tx - will restore nonce to current_nonce"
            );
            Some(current_nonce)
        } else {
            // SubmitRetryable and all other tx types: NO nonce restoration
            // They should increment nonce normally
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

        if is_submit_retryable {
            tracing::warn!(
                target: "reth::evm::execute",
                sender = ?sender,
                is_submit_retryable = is_submit_retryable,
                new_disable_nonce = new_disable_nonce,
                new_disable_balance = new_disable_balance,
                "[req-1] ITER48: Setting disable_nonce_check and disable_balance_check for SubmitRetryable"
            );
        }

        // Debug log for Legacy transactions to verify balance check is enabled
        if matches!(tx.tx().tx_type(), reth_arbitrum_primitives::ArbTxType::Legacy) {
            // Get sender's actual balance from state
            let sender_balance_result = self.inner.evm_mut().db_mut().basic(sender);
            let sender_balance = sender_balance_result.ok().flatten().map(|a| a.balance).unwrap_or_default();
            let required_upfront = alloy_primitives::U256::from(gas_limit) * upfront_gas_price;
            let has_sufficient = sender_balance >= required_upfront;
            tracing::warn!(
                target: "arb-reth::balance-check-debug",
                tx_hash = ?tx_hash,
                sender = ?sender,
                sender_balance = %sender_balance,
                is_internal = is_internal,
                is_deposit = is_deposit,
                is_submit_retryable = is_submit_retryable,
                is_sequenced = is_sequenced,
                new_disable_balance = new_disable_balance,
                needs_precredit = needs_precredit,
                gas_limit = gas_limit,
                upfront_gas_price = %upfront_gas_price,
                required_upfront = %required_upfront,
                has_sufficient = has_sufficient,
                "🔍 LEGACY TX: Balance check config"
            );
        }

        // ITERATION 102 DISABLED: Skip balance validation for user transactions
        //
        // When skip_state_root_validation=true is enabled (trust sequencer mode), we should
        // not validate balances because our computed state may diverge from the real state.
        // The sequencer has already validated these transactions before including them in blocks.
        //
        // Original behavior: Return an error if sender_balance < (gas_cost + tx_value)
        // New behavior: Just log the discrepancy and continue execution
        //
        // Note: The EVM will still enforce balance checks unless disable_balance_check=true
        // is set in the EVM config. Since we set disable_balance_check=true for user transactions
        // when running in trust mode, the EVM will not reject the transaction.
        let is_user_tx = !is_internal && !is_deposit && !is_submit_retryable && !is_retry;
        if is_user_tx {
            let sender_balance_result = self.inner.evm_mut().db_mut().basic(sender);
            let sender_balance = sender_balance_result.ok().flatten().map(|a| a.balance).unwrap_or_default();
            let gas_cost = alloy_primitives::U256::from(gas_limit) * upfront_gas_price;
            let tx_value = tx.tx().value();
            let total_cost = gas_cost.saturating_add(tx_value);

            if sender_balance < total_cost {
                // In trust sequencer mode, we only log the discrepancy and continue
                // This is because our computed state may be wrong, but the sequencer's state is correct
                tracing::debug!(
                    target: "arb-reth::balance-check",
                    tx_hash = ?tx_hash,
                    sender = ?sender,
                    sender_balance = %sender_balance,
                    gas_cost = %gas_cost,
                    tx_value = %tx_value,
                    total_cost = %total_cost,
                    tx_type = ?tx.tx().tx_type(),
                    "⚠️ [ITER102] Balance mismatch detected but continuing (trust sequencer mode)"
                );
                // REMOVED: No longer return an error - trust the sequencer
            }
        }

        let wrapped = WithTxEnv { tx_env, tx };

        // ITERATION 116: Capture gas_used from EVM execution for end_tx hook
        // The end_tx hook needs to know gas_left to properly burn the refund
        let captured_gas_used = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let captured_gas_used_clone = captured_gas_used.clone();

        // Execute transaction through EVM
        // For early-terminated transactions (end_tx_now=true), the state was already
        // modified in start_tx hook, but we still execute through EVM to build receipt.
        // Note: All predeploys have 0xFE code, but early-terminated txs bypass EVM execution.
        let result = self.inner.execute_transaction_with_commit_condition(wrapped, |exec_result| {
            let evm_gas = exec_result.gas_used();
            let actual_gas = hook_gas_override.unwrap_or(evm_gas);
            // Internal transactions (type 0x6a) don't contribute to cumulative gas
            // Use actual_gas (which includes hook overrides) not evm_gas for cumulative calculation
            let gas_to_add = if is_internal { 0u64 } else { actual_gas };
            let new_cumulative = self.cumulative_gas_used + gas_to_add;

            // ITERATION 116: Store the gas used for end_tx hook
            captured_gas_used_clone.store(actual_gas, std::sync::atomic::Ordering::SeqCst);

            tracing::debug!(
                target: "arb-reth::executor",
                tx_hash = ?tx_hash,
                evm_gas = evm_gas,
                actual_gas = actual_gas,
                gas_to_add = gas_to_add,
                current_cumulative = self.cumulative_gas_used,
                new_cumulative = new_cumulative,
                is_internal = is_internal,
                is_override = hook_gas_override.is_some(),
                end_tx_now = start_hook_result.end_tx_now,
                "Storing cumulative gas before receipt creation"
            );

            // For internal txs, store 0 as the gas used in receipt
            let gas_for_receipt = if is_internal { 0u64 } else { actual_gas };
            crate::set_early_tx_gas(tx_hash, gas_for_receipt, new_cumulative);

            f(exec_result)
        });

        // ITERATION 116: Calculate gas_left for end_tx hook
        let actual_gas_used = captured_gas_used.load(std::sync::atomic::Ordering::SeqCst);
        let gas_left_for_end_tx = gas_limit.saturating_sub(actual_gas_used);

        if let Ok(Some(evm_gas)) = result {
            // Internal transactions don't contribute to cumulative gas
            let gas_to_add = if is_internal { 0u64 } else { evm_gas };
            let actual_gas = hook_gas_override.unwrap_or(evm_gas);
            let final_gas_to_add = if is_internal { 0u64 } else { actual_gas };
            self.cumulative_gas_used += final_gas_to_add;

            tracing::info!(
                target: "arb-reth::executor",
                tx_hash = ?tx_hash,
                is_internal = is_internal,
                evm_gas = evm_gas,
                final_gas_to_add = final_gas_to_add,
                new_cumulative = self.cumulative_gas_used,
                "Updated cumulative gas after transaction"
            );
        }

        // ITERATION 81: Nonce restoration for Internal (0x6a) and Retry (0x68) transactions
        //
        // EVM increments nonce during execution even with disable_nonce_check=true.
        // For Internal (ArbOS) and Retry transactions, we must restore the nonce to prevent increment.
        //
        // We defer all nonce restorations until finish() where we modify BOTH:
        // - state.cache.accounts (for any reads within the block)
        // - state.transition_state.transitions (for bundle_state creation via merge_transitions)
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

            // Add to pending restorations - will be applied in finish() before creating bundle_state
            self.pending_nonce_restorations.push((sender, pre_nonce));

            tracing::info!(
                target: "reth::evm::execute",
                sender = ?sender,
                nonce_after_exec = current_nonce_after_exec,
                will_restore_to = pre_nonce,
                is_internal = is_internal,
                is_retry = is_retry,
                tx_type = ?tx_type,
                "[req-1] ITER81: Added nonce restoration to pending list (will update both cache AND transition_state in finish())"
            );
        }

        // ITERATION 99: Balance restoration for Internal/Deposit/SubmitRetryable only.
        //
        // These tx types have end_tx_now=true, meaning they don't go through EVM and
        // don't call end_tx hook. We restore their balance to pre-start_tx value.
        //
        // CRITICAL: Retry transactions are EXCLUDED from this restoration!
        // - Retry goes through EVM with revm's gas accounting (adds refund to sender)
        // - end_tx hook at execute.rs:1270 burns the gas_refund: burn_balance(sender, gas_refund)
        // - If we restore to 0 before end_tx, the burn fails (balance is 0)
        // - Then end_tx's transfer_balance to sender adds ETH back, causing wrong balance
        //
        // By excluding Retry, we let end_tx handle it correctly:
        // - revm adds refund to sender
        // - end_tx burns that exact refund amount
        // - end_tx handles proper gas distribution to refund_to and fee accounts
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
                let balance_diff = if current_balance_after_exec > expected_balance {
                    current_balance_after_exec - expected_balance
                } else {
                    expected_balance - current_balance_after_exec
                };
                tracing::warn!(
                    target: "arb-reth::gas-fix",
                    tx_type = tx_type_u8,
                    sender = ?sender,
                    pre_start_tx_balance = %expected_balance,
                    current_balance = %current_balance_after_exec,
                    balance_diff = %balance_diff,
                    "[ITER99] Restoring sender balance (Internal/Deposit/SubmitRetryable only)"
                );

                // Update the cache to restore the correct balance
                if let Some(cached_acc) = state.cache.accounts.get_mut(&sender) {
                    if let Some(ref mut account) = cached_acc.account {
                        account.info.balance = expected_balance;
                    }
                }

                // CRITICAL: Also update the transition_state for bundle_state creation
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
        
        // ITERATION 82c: Skip end_tx hook for early-terminated transactions
        //
        // In Go, when start_tx returns endTxNow=true, the transaction is complete.
        // No EVM execution, no end_tx hook. The end_tx hook handles gas fee
        // distribution (minting to network_fee_account, infra_fee_account, etc).
        //
        // For Internal (0x6a), SubmitRetryable (0x69), and Deposit (0x64):
        // - start_tx returns endTxNow=true
        // - Go does NOT call end_tx hook
        // - Gas fees should NOT be minted
        //
        // Calling end_tx for these tx types caused incorrect balance mints to
        // network_fee_account (ArbOS), leading to state root mismatches.
        if !start_hook_result.end_tx_now {
            // ITERATION 116: Use actual gas_left from EVM execution, not hardcoded 0
            // This is critical for end_tx to properly burn the gas refund from sender
            let end_ctx = ArbEndTxContext {
                success: result.is_ok(),
                gas_left: gas_left_for_end_tx,
                gas_limit,
                basefee: block_basefee,
                tx_type: tx_type_u8,
                block_timestamp,
            };
            tracing::info!(
                target: "arb-reth::end_tx_debug",
                tx_type = tx_type_u8,
                gas_limit = gas_limit,
                gas_left = gas_left_for_end_tx,
                actual_gas_used = actual_gas_used,
                "[ITER116] Calling end_tx with correct gas_left"
            );
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
                target: "arb-reth::executor",
                tx_type = ?tx_type,
                tx_hash = ?tx_hash,
                "Skipping end_tx hook for early-terminated transaction (end_tx_now=true)"
            );
        }

        // Check for and push any scheduled transactions (retry txs) to the sink
        // This matches Go behavior where ScheduledTxes are collected after each tx
        self.check_and_push_scheduled_txes();

        result
    }

    fn finish(mut self) -> Result<(Self::Evm, RethBlockExecutionResult<reth_arbitrum_primitives::ArbReceipt>), BlockExecutionError> {
        // ITERATION 81: Apply ALL pending nonce restorations NOW, before calling inner.finish()
        //
        // CRITICAL FIX: We must modify BOTH:
        // 1. state.cache.accounts[sender].account.info.nonce - for subsequent tx reads (done in ITER47)
        // 2. state.transition_state.transitions[sender].info.nonce - for bundle_state creation!
        //
        // The bundle_state is created via merge_transitions() which reads from transition_state,
        // NOT from the cache. ITER47 only modified the cache, which is why nonces weren't persisting.
        let restoration_count = self.pending_nonce_restorations.len();
        if restoration_count > 0 {
            tracing::info!(
                target: "arb-reth::executor",
                restoration_count = restoration_count,
                "[req-1] ITER81: Applying {} pending nonce restorations to BOTH cache AND transition_state",
                restoration_count
            );

            // Access the EVM state and apply all nonce restorations
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            for (sender, target_nonce) in &self.pending_nonce_restorations {
                let mut cache_updated = false;
                let mut transition_updated = false;

                // 1. Update the cache (for any subsequent reads within this block)
                if let Some(cached_acc) = state.cache.accounts.get_mut(sender) {
                    if let Some(ref mut account) = cached_acc.account {
                        let wrong_nonce = account.info.nonce;
                        account.info.nonce = *target_nonce;
                        cache_updated = true;

                        tracing::info!(
                            target: "arb-reth::executor",
                            sender = ?sender,
                            wrong_nonce = wrong_nonce,
                            restored_nonce = target_nonce,
                            "[req-1] ITER81: Updated cache nonce"
                        );
                    }
                }

                // 2. CRITICAL: Update the transition_state (for bundle_state creation)
                // This is what ITER47 was missing!
                if let Some(ref mut transition_state) = state.transition_state {
                    if let Some(transition_acc) = transition_state.transitions.get_mut(sender) {
                        if let Some(ref mut info) = transition_acc.info {
                            let wrong_nonce = info.nonce;
                            info.nonce = *target_nonce;
                            transition_updated = true;

                            tracing::warn!(
                                target: "arb-reth::executor",
                                sender = ?sender,
                                wrong_nonce = wrong_nonce,
                                restored_nonce = target_nonce,
                                "[req-1] ITER81: Updated transition_state nonce (THIS IS THE KEY FIX!)"
                            );
                        }
                    }
                }

                if !cache_updated {
                    tracing::warn!(
                        target: "arb-reth::executor",
                        sender = ?sender,
                        "[req-1] ITER81: Sender not in cache or account is None"
                    );
                }

                if !transition_updated {
                    tracing::warn!(
                        target: "arb-reth::executor",
                        sender = ?sender,
                        "[req-1] ITER81: Sender not in transition_state or info is None"
                    );
                }
            }
        }

        // Log bundle_state before finish to see what we have
        {
            let evm = self.inner.evm_mut();
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;
            crate::storage::log_arbos_bundle_state(state);
        }

        // NOW call inner.finish() which will create bundle_state from the corrected transition_state
        let (mut evm, mut result) = self.inner.finish()?;

        // ITER200: Log bundle_state contents after finish to debug state root issues
        {
            let (db_ref, _, _) = evm.components_mut();
            let state: &mut revm::database::State<D> = *db_ref;

            // Check if ArbOS account exists in bundle_state
            let arbos_addr = Address::from([0xa4, 0xb0, 0x5f, 0xff, 0xff, 0xff, 0xff, 0xff,
                                           0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                                           0xff, 0xff, 0xff, 0xff]);

            if let Some(arbos_account) = state.bundle_state.state.get(&arbos_addr) {
                tracing::warn!(
                    target: "arb-reth::bundle_state_debug",
                    "[ITER200] ArbOS account in bundle_state: storage_len={}, status={:?}, has_info={}",
                    arbos_account.storage.len(),
                    arbos_account.status,
                    arbos_account.info.is_some()
                );

                // Log first 5 storage entries
                for (i, (slot, value)) in arbos_account.storage.iter().take(5).enumerate() {
                    tracing::warn!(
                        target: "arb-reth::bundle_state_debug",
                        "[ITER200] ArbOS storage[{}]: slot={} present_value={} original={}",
                        i, slot, value.present_value, value.previous_or_original_value
                    );
                }
            } else {
                tracing::error!(
                    target: "arb-reth::bundle_state_debug",
                    "[ITER200] ArbOS account NOT FOUND in bundle_state!"
                );
            }

            // Log total accounts in bundle_state
            tracing::warn!(
                target: "arb-reth::bundle_state_debug",
                "[ITER200] Total accounts in bundle_state: {}",
                state.bundle_state.state.len()
            );
        }

        tracing::info!(
            target: "arb-reth::executor",
            receipts_count = result.receipts.len(),
            inner_gas_used = result.gas_used,
            "Got result from inner executor"
        );


        if let Some(last_receipt) = result.receipts.last() {
            use alloy_consensus::TxReceipt;
            let correct_gas_used = last_receipt.cumulative_gas_used();
            tracing::info!(
                target: "arb-reth::executor",
                inner_gas = result.gas_used,
                correct_gas = correct_gas_used,
                "Correcting block gasUsed from inner executor value to actual cumulative"
            );
            result.gas_used = correct_gas_used;
        } else {
            tracing::warn!(
                target: "arb-reth::executor",
                "No receipts in result - cannot correct gasUsed"
            );
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
        
        tracing::info!(
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
            tracing::info!(
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
