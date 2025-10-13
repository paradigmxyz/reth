use alloy_primitives::{Address, U256, B256, Bytes};
use arb_alloy_util::l1_pricing::L1PricingState as AlloyL1PricingState;
use crate::retryables::{Retryables, DefaultRetryables, RetryableCreateParams, RetryableAction, RetryableTicketId};
use reth_arbitrum_primitives::{ArbTxType, ArbTransactionSigned, ArbTypedTransaction};
use alloy_consensus::Transaction as AlloyCoinbaseTransaction;
use revm::Database;
use crate::arbosstate::ArbosState;

pub struct ArbStartTxContext {
    pub sender: Address,
    pub nonce: u64,
    pub l1_base_fee: U256,
    pub calldata_len: usize,
    pub coinbase: Address,
    pub executed_on_chain: bool,
    pub is_eth_call: bool,
    pub tx_type: u8,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_limit: u64,
    pub basefee: U256,
    pub ticket_id: Option<B256>,
    pub refund_to: Option<Address>,
    pub gas_fee_cap: Option<U256>,
    pub max_refund: Option<U256>,
    pub submission_fee_refund: Option<U256>,
    pub tx_hash: B256,
    pub deposit_value: Option<U256>,
    pub retry_value: Option<U256>,
    pub retry_to: Option<Address>,
    pub retry_data: Option<Vec<u8>>,
    pub beneficiary: Option<Address>,
    pub max_submission_fee: Option<U256>,
    pub fee_refund_addr: Option<Address>,
    pub block_timestamp: u64,
    pub data: Option<Vec<u8>>,
}

pub struct ArbGasChargingContext {
    pub intrinsic_gas: u64,
    pub calldata: Vec<u8>,
    pub tx_bytes: Vec<u8>,
    pub basefee: U256,
    pub is_executed_on_chain: bool,
    pub skip_l1_charging: bool,
    pub poster: Address,
    pub gas_remaining: u64,
    pub is_ethcall: bool,
}

pub struct ArbEndTxContext {
    pub success: bool,
    pub gas_left: u64,
    pub gas_limit: u64,
    pub basefee: U256,
    pub tx_type: u8,
    pub block_timestamp: u64,
}

pub struct StartTxHookResult {
    pub end_tx_now: bool,
    pub gas_used: u64,
    pub error: Option<String>,
}

impl Default for StartTxHookResult {
    fn default() -> Self {
        Self {
            end_tx_now: false,
            gas_used: 0,
            error: None,
        }
    }
}

pub trait ArbOsHooks {
    fn start_tx<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbStartTxContext,
    ) -> StartTxHookResult;
    
    fn gas_charging<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbGasChargingContext,
    ) -> (Address, Result<(), ()>);
    
    fn end_tx<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbEndTxContext,
    );
    
    fn nonrefundable_gas(&self, state: &ArbTxProcessorState) -> u64;
    fn held_gas(&self, state: &ArbTxProcessorState) -> u64;
    
    fn scheduled_txes<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &ArbTxProcessorState,
        logs: &[alloy_primitives::Log],
        chain_id: u64,
        block_timestamp: u64,
        basefee: U256,
    ) -> Vec<Vec<u8>>;
    
    fn l1_block_number<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
    ) -> Result<u64, ()>;
    
    fn l1_block_hash<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        l1_block_number: u64,
    ) -> Result<B256, ()>;
    
    fn drop_tip(&self, state: &ArbTxProcessorState) -> bool;
    fn get_paid_gas_price(&self, state: &ArbTxProcessorState, evm_gas_price: U256, basefee: U256) -> U256;
    fn gas_price_op(&self, state: &ArbTxProcessorState, evm_gas_price: U256, basefee: U256) -> U256;
    fn fill_receipt_info(&self, state: &ArbTxProcessorState) -> u64;
    fn msg_is_non_mutating(&self, ctx: &ArbStartTxContext) -> bool;
    fn is_calldata_pricing_increase_enabled<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &ArbTxProcessorState,
    ) -> bool;
    
    fn push_contract(&self, state: &mut ArbTxProcessorState, contract_addr: Address, is_delegate_or_callcode: bool);
    fn pop_contract(&self, state: &mut ArbTxProcessorState, is_delegate_or_callcode: bool);
    fn execute_wasm<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &ArbTxProcessorState,
        contract_addr: Address,
        input: &[u8],
    ) -> Result<Vec<u8>, Vec<u8>>;
}

#[derive(Default, Clone)]
pub struct DefaultArbOsHooks;

impl DefaultArbOsHooks {
    fn process_parent_block_hash<D: Database>(
        state_db: &mut revm::database::State<D>,
        prev_hash: B256,
    ) {
        const HISTORY_STORAGE_ADDRESS: Address = Address::new([
            0x00, 0x00, 0xF9, 0x08, 0x27, 0xF1, 0xC5, 0x3a,
            0x10, 0xcb, 0x7A, 0x02, 0x33, 0x5B, 0x17, 0x53,
            0x20, 0x00, 0x29, 0x35,
        ]);
        
        use revm_state::EvmStorageSlot;
        use revm_database::{BundleAccount, AccountStatus};
        use revm_state::AccountInfo;
        
        if !state_db.bundle_state.state.contains_key(&HISTORY_STORAGE_ADDRESS) {
            let info = match state_db.basic(HISTORY_STORAGE_ADDRESS) {
                Ok(Some(account_info)) => Some(account_info),
                _ => Some(AccountInfo {
                    balance: U256::ZERO,
                    nonce: 0,
                    code_hash: alloy_primitives::keccak256([]),
                    code: None,
                }),
            };
            
            let acc = BundleAccount {
                info,
                storage: std::collections::HashMap::default(),
                original_info: None,
                status: AccountStatus::Changed,
            };
            state_db.bundle_state.state.insert(HISTORY_STORAGE_ADDRESS, acc);
        }
        
        let slot = U256::from_be_bytes(prev_hash.0);
        let value_u256 = U256::from_be_bytes(prev_hash.0);
        
        if let Some(acc) = state_db.bundle_state.state.get_mut(&HISTORY_STORAGE_ADDRESS) {
            acc.storage.insert(
                slot,
                EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
            );
        }
    }
    
    fn compress_tx_data(data: &[u8], level: u32) -> Result<Vec<u8>, std::io::Error> {
        let mut compressed = Vec::new();
        let params = brotli::enc::BrotliEncoderParams {
            quality: level as i32,
            lgwin: 22,
            ..Default::default()
        };
        let mut compressor = brotli::CompressorWriter::with_params(
            &mut compressed,
            4096,
            &params
        );
        std::io::Write::write_all(&mut compressor, data)?;
        drop(compressor);
        Ok(compressed)
    }

    fn get_poster_gas(basefee: U256, poster_cost: U256) -> u64 {
        if basefee.is_zero() {
            return 0;
        }
        let q = poster_cost.checked_div(basefee).unwrap_or_default();
        q.try_into().unwrap_or(u64::MAX)
    }

    pub fn mint_balance<D>(state: &mut revm::database::State<D>, address: Address, amount: U256) 
    where
        D: revm::Database,
    {
        if amount.is_zero() {
            return;
        }
        let _ = state.load_cache_account(address);
        let amount_u128: u128 = amount.try_into().unwrap_or(u128::MAX);
        let _ = state.increment_balances(core::iter::once((address, amount_u128)));
    }

    pub fn transfer_balance<D>(
        state: &mut revm::database::State<D>,
        from: Address,
        to: Address,
        amount: U256,
    ) -> Result<(), ()>
    where
        D: revm::Database,
    {
        if amount.is_zero() {
            return Ok(());
        }
        
        let _ = state.load_cache_account(from);
        let _ = state.load_cache_account(to);
        
        let from_account = match state.basic(from) {
            Ok(info) => info,
            Err(_) => return Err(()),
        };
        let from_balance = from_account.map(|i| U256::from(i.balance)).unwrap_or_default();
        
        if from_balance < amount {
            return Err(());
        }
        
        let amount_u128: u128 = amount.try_into().unwrap_or(u128::MAX);
        let _ = state.increment_balances(core::iter::once((from, amount_u128.wrapping_neg())));
        let _ = state.increment_balances(core::iter::once((to, amount_u128)));
        Ok(())
    }

    pub fn burn_balance<D>(
        state: &mut revm::database::State<D>,
        from: Address,
        amount: U256,
    ) -> Result<(), ()>
    where
        D: revm::Database,
    {
        if amount.is_zero() {
            return Ok(());
        }
        
        let _ = state.load_cache_account(from);
        
        let from_account = match state.basic(from) {
            Ok(info) => info,
            Err(_) => return Err(()),
        };
        let from_balance = from_account.map(|i| U256::from(i.balance)).unwrap_or_default();
        
        if from_balance < amount {
            return Err(());
        }
        
        let amount_u128: u128 = amount.try_into().unwrap_or(u128::MAX);
        let _ = state.increment_balances(core::iter::once((from, amount_u128.wrapping_neg())));
        Ok(())
    }
    
    fn take_funds(available: &mut U256, amount: U256) -> U256 {
        let taken = (*available).min(amount);
        *available = available.saturating_sub(taken);
        taken
    }

    pub fn execute_deposit<D>(
        state_db: &mut revm::database::State<D>,
        from: Address,
        to: Address,
        value: U256,
    ) -> Result<(), ()>
    where
        D: revm::Database,
    {
        if value.is_zero() {
            return Ok(());
        }
        
        Self::mint_balance(state_db, from, value);
        
        Self::transfer_balance(state_db, from, to, value)
    }
    
    pub fn execute_submit_retryable<D>(
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        tx: &ArbTransactionSigned,
        tx_hash: B256,
        l1_base_fee: U256,
        block_timestamp: u64,
    ) -> Result<(), ()>
    where
        D: revm::Database,
    {
        let tx_inner = match &**tx {
            ArbTypedTransaction::SubmitRetryable(inner) => inner,
            _ => return Err(()),
        };

        let from = tx_inner.from;
        let deposit_value = tx_inner.deposit_value;
        let retry_value = tx_inner.retry_value;
        let max_submission_fee = tx_inner.max_submission_fee;
        let fee_refund_addr = tx_inner.fee_refund_addr;
        let beneficiary = tx_inner.beneficiary;
        let retry_to = tx_inner.retry_to;
        let retry_data = &tx_inner.retry_data;
        let gas_fee_cap = tx_inner.gas_fee_cap;
        let gas_limit = tx_inner.gas;

        let ticket_id = RetryableTicketId(tx_hash.0);
        let escrow_bytes = arb_alloy_util::retryables::escrow_address_from_ticket(ticket_id.0);
        let escrow = Address::from(escrow_bytes);
        let network_fee_account = state.network_fee_account;

        let mut available_refund = deposit_value;
        let _retry_value_taken = Self::take_funds(&mut available_refund, retry_value);

        Self::mint_balance(state_db, from, deposit_value);

        let after_mint_account = match state_db.basic(from) {
            Ok(info) => info,
            Err(_) => {
                tracing::error!("execute_submit_retryable: failed to get account info for {:?}", from);
                return Err(());
            }
        };
        let balance_after_mint = after_mint_account.map(|i| U256::from(i.balance)).unwrap_or_default();

        if balance_after_mint < max_submission_fee {
            tracing::error!("execute_submit_retryable: insufficient balance balance={} max_submission_fee={}", balance_after_mint, max_submission_fee);
            return Err(());
        }

        let submission_fee = U256::from(arb_alloy_util::retryables::retryable_submission_fee(
            retry_data.len(),
            l1_base_fee.try_into().unwrap_or(u128::MAX),
        ));

        if max_submission_fee < submission_fee {
            tracing::error!("execute_submit_retryable: max_submission_fee too low max={} required={}", max_submission_fee, submission_fee);
            return Err(());
        }

        if let Err(_) = Self::transfer_balance(state_db, from, network_fee_account, submission_fee) {
            tracing::error!("execute_submit_retryable: failed to transfer submission_fee from={:?} to={:?} amount={}", from, network_fee_account, submission_fee);
            return Err(());
        }
        let withheld_submission_fee = Self::take_funds(&mut available_refund, submission_fee);

        let submission_fee_refund = Self::take_funds(
            &mut available_refund,
            max_submission_fee.saturating_sub(submission_fee),
        );
        let _ = Self::transfer_balance(state_db, from, fee_refund_addr, submission_fee_refund);

        if let Err(_) = Self::transfer_balance(state_db, from, escrow, retry_value) {
            tracing::error!("execute_submit_retryable: failed to transfer retry_value from={:?} to={:?} amount={}", from, escrow, retry_value);
            let _ = Self::transfer_balance(state_db, network_fee_account, from, submission_fee);
            let _ = Self::transfer_balance(state_db, from, fee_refund_addr, withheld_submission_fee);
            return Err(());
        }

        let timeout = block_timestamp + 604800;

        let params = RetryableCreateParams {
            sender: from,
            beneficiary,
            call_to: retry_to.unwrap_or(Address::ZERO),
            call_data: retry_data.clone(),
            l1_base_fee,
            submission_fee,
            max_submission_cost: max_submission_fee,
            max_gas: U256::from(gas_limit),
            gas_price_bid: gas_fee_cap,
        };

        use crate::retryables::{RetryableState, DefaultRetryables};
        use alloy_primitives::B256;
        
        let retryable_state = RetryableState::new(state_db as *mut _, B256::ZERO);
        let _ticket = retryable_state.create_retryable(state_db as *mut _, ticket_id, params, block_timestamp);
        
        Ok(())
    }
}

impl ArbOsHooks for DefaultArbOsHooks {
    fn start_tx<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbStartTxContext,
    ) -> StartTxHookResult {
        state.delayed_inbox = ctx.coinbase != Address::ZERO;
        
        match ctx.tx_type {
            0x64 => {
                let to = match ctx.to {
                    Some(addr) => addr,
                    None => {
                        return StartTxHookResult {
                            end_tx_now: true,
                            gas_used: 0,
                            error: Some("eth deposit has no To address".to_string()),
                        };
                    }
                };
                
                Self::mint_balance(state_db, ctx.sender, ctx.value);
                
                let _ = Self::transfer_balance(state_db, ctx.sender, to, ctx.value);
                
                StartTxHookResult {
                    end_tx_now: true,
                    gas_used: 0,
                    error: None,
                }
            }
            
            0x6A => {
                const ARBOS_ADDR: Address = Address::new([
                    0xA4, 0xB0, 0x5F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
                    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
                    0xFF, 0xFF, 0xFF, 0xFF,
                ]);
                
                if ctx.sender != ARBOS_ADDR {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("internal tx not from arbAddress".to_string()),
                    };
                }
                
                use crate::internal_tx::unpack_internal_tx_data_start_block;
                
                let data = ctx.data.as_deref().unwrap_or(&[]);
                let internal_data = match unpack_internal_tx_data_start_block(data) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("Failed to unpack internal tx data: {}", e);
                        return StartTxHookResult {
                            end_tx_now: true,
                            gas_used: 0,
                            error: Some(format!("invalid internal tx data: {}", e)),
                        };
                    }
                };
                
                let arbos_version = if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                    arbos_state.arbos_version
                } else {
                    11
                };
                
                let prev_hash = B256::ZERO;
                
                if arbos_version >= 40 {
                    Self::process_parent_block_hash(state_db, prev_hash);
                }
                
                let blockhashes_storage = crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(6),
                );
                let blockhashes = crate::blockhash::Blockhashes::open(blockhashes_storage);
                
                let old_l1_block_number = blockhashes.l1_block_number().unwrap_or(0);
                let l1_block_number = internal_data.l1_block_number;
                
                if l1_block_number > old_l1_block_number {
                    if let Err(e) = blockhashes.record_new_l1_block(
                        l1_block_number - 1,
                        prev_hash,
                        arbos_version,
                    ) {
                        tracing::error!("Failed to record new L1 block: {:?}", e);
                    }
                }
                
                let retryable_storage = crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(2),
                );
                let retryable_state = crate::retryables::RetryableState::new(
                    state_db as *mut _,
                    retryable_storage.base_key,
                );
                
                let current_time = ctx.block_timestamp;
                let _ = retryable_state.try_to_reap_one_retryable(current_time, state_db as *mut _);
                let _ = retryable_state.try_to_reap_one_retryable(current_time, state_db as *mut _);
                
                let l2_pricing = crate::l2_pricing::L2PricingState::open(crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(1),
                ));
                
                let l2_base_fee = l2_pricing.get_base_fee_l2().unwrap_or(U256::ZERO);
                
                if let Err(e) = l2_pricing.update_pricing_model(l2_base_fee, internal_data.time_passed) {
                    tracing::error!("Failed to update L2 pricing model: {:?}", e);
                }
                
                if let Ok(mut arbos_state) = crate::arbosstate::ArbosState::open(state_db as *mut _) {
                    if let Err(e) = arbos_state.upgrade_arbos_version_if_necessary(current_time, state_db) {
                        tracing::error!("Failed to upgrade ArbOS version: {:?}", e);
                    }
                }
                
                StartTxHookResult {
                    end_tx_now: true,
                    gas_used: 0,
                    error: None,
                }
            }
            
            0x68 => {
                let ticket_id = match ctx.ticket_id {
                    Some(id) => id,
                    None => {
                        return StartTxHookResult {
                            end_tx_now: true,
                            gas_used: 0,
                            error: Some("retry tx missing ticket_id".to_string()),
                        };
                    }
                };
                
                let retryable_storage = crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(2),
                );
                let retryable_state = crate::retryables::RetryableState::new(
                    state_db as *mut _,
                    retryable_storage.base_key,
                );
                
                let ticket_id_struct = crate::retryables::RetryableTicketId(ticket_id.0);
                let current_time = ctx.block_timestamp;
                
                if let Some(retryable) = retryable_state.open_retryable(
                    state_db as *mut _,
                    &ticket_id_struct,
                    current_time,
                ) {
                    let _ = retryable.increment_tries();
                } else {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("retryable not found or expired".to_string()),
                    };
                }
                
                use arb_alloy_util::retryables::escrow_address_from_ticket;
                let escrow = Address::from_slice(&escrow_address_from_ticket(ticket_id.0));
                
                if let Err(_) = Self::transfer_balance(state_db, escrow, ctx.sender, ctx.value) {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("failed to transfer from escrow".to_string()),
                    };
                }
                
                let prepaid = ctx.basefee.saturating_mul(U256::from(ctx.gas_limit));
                Self::mint_balance(state_db, ctx.sender, prepaid);
                
                let refund_to = ctx.refund_to.unwrap_or(ctx.sender);
                let gas_fee_cap = ctx.gas_fee_cap.unwrap_or(ctx.basefee);
                let max_refund = ctx.max_refund.unwrap_or(U256::ZERO);
                let submission_fee_refund = ctx.submission_fee_refund.unwrap_or(U256::ZERO);
                
                state.current_retry_data = Some(CurrentRetryData {
                    ticket_id,
                    from: ctx.sender,
                    refund_to,
                    value: ctx.value,
                    gas_fee_cap,
                    max_refund,
                    submission_fee_refund,
                });
                
                StartTxHookResult::default()
            }
            
            0x69 => {
                let deposit_value = ctx.deposit_value.unwrap_or(U256::ZERO);
                let retry_value = ctx.retry_value.unwrap_or(U256::ZERO);
                let retry_to = ctx.retry_to.unwrap_or(Address::ZERO);
                let retry_data = ctx.retry_data.as_ref().map(|d| d.as_slice()).unwrap_or(&[]);
                let beneficiary = ctx.beneficiary.unwrap_or(ctx.sender);
                let max_submission_fee = ctx.max_submission_fee.unwrap_or(U256::ZERO);
                let fee_refund_addr = ctx.fee_refund_addr.unwrap_or(ctx.sender);
                let gas_fee_cap = ctx.gas_fee_cap.unwrap_or(ctx.basefee);
                
                let ticket_id = ctx.tx_hash;
                
                let mut available_refund = deposit_value;
                available_refund = available_refund.saturating_sub(retry_value);
                
                Self::mint_balance(state_db, ctx.sender, deposit_value);
                
                use arb_alloy_util::retryables::escrow_address_from_ticket;
                let escrow = Address::from_slice(&escrow_address_from_ticket(ticket_id.0));
                
                let balance_after_mint = match state_db.basic(ctx.sender) {
                    Ok(Some(acc)) => U256::from(acc.balance),
                    _ => U256::ZERO,
                };
                
                if balance_after_mint < max_submission_fee {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some(format!("insufficient funds for max submission fee: have {} want {}", balance_after_mint, max_submission_fee)),
                    };
                }
                
                let submission_fee = arb_alloy_util::retryables::retryable_submission_fee(retry_data.len(), ctx.l1_base_fee.try_into().unwrap_or(0));
                let submission_fee_u256 = U256::from(submission_fee);
                
                if max_submission_fee < submission_fee_u256 {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some(format!("max submission fee {} < actual {}", max_submission_fee, submission_fee_u256)),
                    };
                }
                
                if let Err(_) = Self::transfer_balance(state_db, ctx.sender, state.network_fee_account, submission_fee_u256) {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("failed to transfer submission fee".to_string()),
                    };
                }
                let withheld_submission_fee = Self::take_funds(&mut available_refund, submission_fee_u256);
                
                let submission_fee_refund = Self::take_funds(&mut available_refund, max_submission_fee.saturating_sub(submission_fee_u256));
                let _ = Self::transfer_balance(state_db, ctx.sender, fee_refund_addr, submission_fee_refund);
                
                if let Err(_) = Self::transfer_balance(state_db, ctx.sender, escrow, retry_value) {
                    let _ = Self::transfer_balance(state_db, state.network_fee_account, ctx.sender, submission_fee_u256);
                    let _ = Self::transfer_balance(state_db, ctx.sender, fee_refund_addr, withheld_submission_fee);
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("failed to escrow callvalue".to_string()),
                    };
                }
                
                let balance = match state_db.basic(ctx.sender) {
                    Ok(Some(acc)) => U256::from(acc.balance),
                    _ => U256::ZERO,
                };
                
                let effective_base_fee = ctx.basefee;
                let usergas = ctx.gas_limit;
                
                let max_gas_cost = gas_fee_cap.saturating_mul(U256::from(usergas));
                let max_fee_per_gas_too_low = gas_fee_cap < effective_base_fee;
                
                if balance < max_gas_cost || usergas < 21000 || max_fee_per_gas_too_low {
                    let gas_cost_refund = Self::take_funds(&mut available_refund, max_gas_cost);
                    let _ = Self::transfer_balance(state_db, ctx.sender, fee_refund_addr, gas_cost_refund);
                    
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: None,
                    };
                }
                
                let gascost = effective_base_fee.saturating_mul(U256::from(usergas));
                let mut network_cost = gascost;
                
                if state.arbos_version >= 11 && !state.infra_fee_account.is_zero() {
                    let infra_fee = state.min_base_fee.min(effective_base_fee);
                    let infra_cost = infra_fee.saturating_mul(U256::from(usergas));
                    let infra_cost_taken = Self::take_funds(&mut network_cost, infra_cost);
                    
                    if let Err(_) = Self::transfer_balance(state_db, ctx.sender, state.infra_fee_account, infra_cost_taken) {
                        tracing::error!("failed to transfer gas cost to infrastructure fee account");
                        return StartTxHookResult {
                            end_tx_now: true,
                            gas_used: 0,
                            error: None,
                        };
                    }
                }
                
                if network_cost > U256::ZERO {
                    if let Err(_) = Self::transfer_balance(state_db, ctx.sender, state.network_fee_account, network_cost) {
                        tracing::error!("failed to transfer gas cost to network fee account");
                        return StartTxHookResult {
                            end_tx_now: true,
                            gas_used: 0,
                            error: None,
                        };
                    }
                }
                
                let withheld_gas_funds = Self::take_funds(&mut available_refund, gascost);
                let mut gas_price_refund = gas_fee_cap.saturating_sub(effective_base_fee).saturating_mul(U256::from(usergas));
                gas_price_refund = Self::take_funds(&mut available_refund, gas_price_refund);
                let _ = Self::transfer_balance(state_db, ctx.sender, fee_refund_addr, gas_price_refund);
                
                available_refund = available_refund.saturating_add(withheld_gas_funds);
                available_refund = available_refund.saturating_add(withheld_submission_fee);
                
                const ARB_RETRYABLE_TX_ADDRESS: Address = Address::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x6e]);
                const TICKET_CREATED_TOPIC: [u8; 32] = [0x7c, 0x79, 0x3c, 0xce, 0xd5, 0x74, 0x3d, 0xc5, 0xf5, 0x31, 0xbb, 0xe2, 0xbf, 0xb5, 0xa9, 0xfa, 0x3f, 0x40, 0xad, 0xef, 0x29, 0x23, 0x1e, 0x6a, 0xb1, 0x65, 0xc0, 0x8a, 0x29, 0xe3, 0xdd, 0x89];
                const REDEEM_SCHEDULED_TOPIC: [u8; 32] = [0x5c, 0xcd, 0x00, 0x95, 0x02, 0x50, 0x9c, 0xf2, 0x87, 0x62, 0xc6, 0x78, 0x58, 0x99, 0x4d, 0x85, 0xb1, 0x63, 0xbb, 0x6e, 0x45, 0x1f, 0x5e, 0x9d, 0xf7, 0xc5, 0xe1, 0x8c, 0x9c, 0x2e, 0x12, 0x3e];
                
                crate::log_sink::push(ARB_RETRYABLE_TX_ADDRESS, &[TICKET_CREATED_TOPIC, ticket_id.0], &[]);
                
                let retryable_storage = crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(2),
                );
                let retryable_state = crate::retryables::RetryableState::new(
                    state_db as *mut _,
                    retryable_storage.base_key,
                );
                
                let timeout = ctx.block_timestamp + crate::retryables::RETRYABLE_LIFETIME_SECONDS;
                
                use crate::retryables::RetryableCreateParams;
                let create_params = RetryableCreateParams {
                    sender: ctx.sender,
                    beneficiary,
                    call_to: retry_to,
                    call_data: Bytes::from(retry_data.to_vec()),
                    l1_base_fee: ctx.l1_base_fee,
                    submission_fee: submission_fee_u256,
                    max_submission_cost: max_submission_fee,
                    max_gas: U256::from(usergas),
                    gas_price_bid: gas_fee_cap,
                };
                
                let ticket_id_struct = crate::retryables::RetryableTicketId(ticket_id.0);
                let _ticket = retryable_state.create_retryable(
                    state_db as *mut _,
                    ticket_id_struct,
                    create_params,
                    ctx.block_timestamp,
                );
                
                let retry_tx_nonce = 0u64;
                let retry_tx_hash = B256::ZERO;
                let sequence_num_bytes: [u8; 32] = {
                    let mut bytes = [0u8; 32];
                    bytes[24..].copy_from_slice(&retry_tx_nonce.to_be_bytes());
                    bytes
                };
                
                let mut redeem_data = Vec::new();
                redeem_data.extend_from_slice(&[0u8; 24]);
                redeem_data.extend_from_slice(&usergas.to_be_bytes());
                redeem_data.extend_from_slice(&[0u8; 12]);
                redeem_data.extend_from_slice(fee_refund_addr.as_slice());
                let max_refund_bytes: [u8; 32] = available_refund.to_be_bytes();
                redeem_data.extend_from_slice(&max_refund_bytes);
                let submission_fee_bytes: [u8; 32] = submission_fee_u256.to_be_bytes();
                redeem_data.extend_from_slice(&submission_fee_bytes);
                
                crate::log_sink::push(ARB_RETRYABLE_TX_ADDRESS, &[REDEEM_SCHEDULED_TOPIC, ticket_id.0, retry_tx_hash.0, sequence_num_bytes], &redeem_data);
                
                StartTxHookResult {
                    end_tx_now: true,
                    gas_used: usergas,
                    error: None,
                }
            }
            
            _ => StartTxHookResult::default(),
        }
    }

    fn gas_charging<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbGasChargingContext,
    ) -> (Address, Result<(), ()>) {
        let mut gas_needed_to_start_evm = 0u64;
        let tip_recipient = state.network_fee_account;
        
        if ctx.basefee.is_zero() || ctx.skip_l1_charging {
            if !ctx.is_ethcall && ctx.gas_remaining > 0 {
                if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                    if let Ok(gas_available) = arbos_state.l2_pricing_state.get_per_block_gas_limit() {
                        if ctx.gas_remaining > gas_available {
                            state.compute_hold_gas = ctx.gas_remaining.saturating_sub(gas_available);
                        }
                    }
                }
            }
            return (tip_recipient, Ok(()));
        }
        
        if ctx.poster != crate::l1_pricing::BATCH_POSTER_ADDRESS {
            if !ctx.is_ethcall && ctx.gas_remaining > 0 {
                if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                    if let Ok(gas_available) = arbos_state.l2_pricing_state.get_per_block_gas_limit() {
                        if ctx.gas_remaining > gas_available {
                            state.compute_hold_gas = ctx.gas_remaining.saturating_sub(gas_available);
                        }
                    }
                }
            }
            return (tip_recipient, Ok(()));
        }
        
        let brotli_level = state.brotli_compression_level as u64;
        
        if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
            let (poster_cost, calldata_units) = match arbos_state.l1_pricing_state.get_poster_data_cost(
                &ctx.tx_bytes,
                ctx.poster,
                brotli_level
            ) {
                Ok(result) => result,
                Err(_) => {
                    tracing::error!("Failed to get poster data cost");
                    return (tip_recipient, Err(()));
                }
            };
            
            if calldata_units > 0 {
                let _ = arbos_state.l1_pricing_state.add_to_units_since_update(calldata_units);
            }
            
            let poster_gas = Self::get_poster_gas(ctx.basefee, poster_cost);
            state.poster_gas = poster_gas;
            state.poster_fee = ctx.basefee.saturating_mul(U256::from(poster_gas));
            gas_needed_to_start_evm = poster_gas;
        }
        
        if ctx.gas_remaining < gas_needed_to_start_evm {
            tracing::debug!("Insufficient gas for L1 calldata costs");
            return (tip_recipient, Err(()));
        }
        
        let gas_remaining_after_l1 = ctx.gas_remaining.saturating_sub(gas_needed_to_start_evm);
        
        if !ctx.is_ethcall {
            if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                if let Ok(gas_available) = arbos_state.l2_pricing_state.get_per_block_gas_limit() {
                    if gas_remaining_after_l1 > gas_available {
                        state.compute_hold_gas = gas_remaining_after_l1.saturating_sub(gas_available);
                    }
                }
            }
        }
        
        (tip_recipient, Ok(()))
    }

    fn end_tx<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        ctx: &ArbEndTxContext,
    ) {
        if ctx.tx_type == 0x68 {
            if ctx.gas_left > ctx.gas_limit {
                tracing::error!("Tx refunds gas after computation - impossible");
                return;
            }
            let gas_used = ctx.gas_limit.saturating_sub(ctx.gas_left);
            
            if let Some(retry_data) = &state.current_retry_data {
                let effective_base_fee = retry_data.gas_fee_cap;
                
                let gas_refund = effective_base_fee.saturating_mul(U256::from(ctx.gas_left));
                if gas_refund > U256::ZERO {
                    let _ = Self::burn_balance(state_db, retry_data.from, gas_refund);
                }
                
                let mut max_refund = retry_data.max_refund;
                
                if ctx.success {
                    let submission_fee_refund = Self::take_funds(&mut max_refund, retry_data.submission_fee_refund);
                    if submission_fee_refund > U256::ZERO {
                        let _ = Self::transfer_balance(state_db, state.network_fee_account, retry_data.refund_to, submission_fee_refund);
                    }
                } else {
                    let _ = Self::take_funds(&mut max_refund, retry_data.submission_fee_refund);
                }
                
                let gas_cost = effective_base_fee.saturating_mul(U256::from(gas_used));
                let _ = Self::take_funds(&mut max_refund, gas_cost);
                
                let mut network_refund = gas_refund;
                if state.arbos_version >= 11 && !state.infra_fee_account.is_zero() {
                    let infra_fee = state.min_base_fee.min(effective_base_fee);
                    let infra_refund = infra_fee.saturating_mul(U256::from(ctx.gas_left));
                    let infra_refund_taken = Self::take_funds(&mut network_refund, infra_refund);
                    
                    if infra_refund_taken > U256::ZERO {
                        let to_refund_addr = Self::take_funds(&mut max_refund, infra_refund_taken);
                        let _ = Self::transfer_balance(state_db, state.infra_fee_account, retry_data.refund_to, to_refund_addr);
                        let remainder = infra_refund_taken.saturating_sub(to_refund_addr);
                        if remainder > U256::ZERO {
                            let _ = Self::transfer_balance(state_db, state.infra_fee_account, retry_data.from, remainder);
                        }
                    }
                }
                
                if network_refund > U256::ZERO {
                    let to_refund_addr = Self::take_funds(&mut max_refund, network_refund);
                    let _ = Self::transfer_balance(state_db, state.network_fee_account, retry_data.refund_to, to_refund_addr);
                    let remainder = network_refund.saturating_sub(to_refund_addr);
                    if remainder > U256::ZERO {
                        let _ = Self::transfer_balance(state_db, state.network_fee_account, retry_data.from, remainder);
                    }
                }
                
                if ctx.success {
                    let retryable_storage = crate::storage::Storage::new(
                        state_db as *mut _,
                        crate::arbosstate::arbos_state_subspace(2),
                    );
                    let retryable_state = crate::retryables::RetryableState::new(
                        state_db as *mut _,
                        retryable_storage.base_key,
                    );
                    
                    let ticket_id_struct = crate::retryables::RetryableTicketId(retry_data.ticket_id.0);
                    if let Some(retryable) = retryable_state.open_retryable(
                        state_db as *mut _,
                        &ticket_id_struct,
                        ctx.block_timestamp,
                    ) {
                        use arb_alloy_util::retryables::escrow_address_from_ticket;
                        let escrow = Address::from_slice(&escrow_address_from_ticket(retry_data.ticket_id.0));
                        
                        if let Some(beneficiary) = retryable.get_beneficiary() {
                            let escrow_balance = match state_db.basic(escrow) {
                                Ok(Some(acc)) => U256::from(acc.balance),
                                _ => U256::ZERO,
                            };
                            if escrow_balance > U256::ZERO {
                                let _ = Self::transfer_balance(state_db, escrow, beneficiary, escrow_balance);
                            }
                        }
                        
                        let _ = retryable.deactivate();
                    }
                } else {
                    use arb_alloy_util::retryables::escrow_address_from_ticket;
                    let escrow = Address::from_slice(&escrow_address_from_ticket(retry_data.ticket_id.0));
                    let _ = Self::transfer_balance(state_db, retry_data.from, escrow, retry_data.value);
                }
                
                if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                    let gas_used_i64 = -(gas_used as i64);
                    let _ = arbos_state.l2_pricing_state.add_to_gas_pool(gas_used_i64);
                }
            }
            return;
        }
        
        if ctx.gas_left > ctx.gas_limit {
            tracing::error!("Tx refunds gas after computation - impossible");
            return;
        }
        let gas_used = ctx.gas_limit.saturating_sub(ctx.gas_left);
        
        let total_cost = ctx.basefee.saturating_mul(U256::from(gas_used));
        
        let mut compute_cost = total_cost.saturating_sub(state.poster_fee);
        if compute_cost > total_cost {
            tracing::error!("total cost < poster cost, gasUsed={} basefee={} posterFee={}", 
                gas_used, ctx.basefee, state.poster_fee);
            state.poster_fee = U256::ZERO;
            compute_cost = total_cost;
        }
        
        if state.arbos_version > 4 && !state.infra_fee_account.is_zero() {
            let infra_fee = state.min_base_fee.min(ctx.basefee);
            
            let compute_gas = gas_used.saturating_sub(state.poster_gas);
            
            let infra_compute_cost = infra_fee.saturating_mul(U256::from(compute_gas));
            
            Self::mint_balance(state_db, state.infra_fee_account, infra_compute_cost);
            
            compute_cost = compute_cost.saturating_sub(infra_compute_cost);
        }
        
        if compute_cost > U256::ZERO {
            Self::mint_balance(state_db, state.network_fee_account, compute_cost);
        }
        
        let poster_fee_dest = if state.arbos_version >= 2 {
            crate::l1_pricing::L1_PRICER_FUNDS_POOL_ADDRESS
        } else {
            Address::ZERO
        };
        
        if state.poster_fee > U256::ZERO && !poster_fee_dest.is_zero() {
            Self::mint_balance(state_db, poster_fee_dest, state.poster_fee);
            
            if state.arbos_version >= 10 {
                if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                    let _ = arbos_state.l1_pricing_state.add_to_l1_fees_available(state.poster_fee);
                }
            }
        }
        
        if ctx.basefee > U256::ZERO {
            let compute_gas = if gas_used > state.poster_gas {
                gas_used - state.poster_gas
            } else {
                tracing::error!("total gas used < poster gas component, gasUsed={} posterGas={}", 
                    gas_used, state.poster_gas);
                gas_used
            };
            
            if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
                let compute_gas_i64 = -(compute_gas as i64);
                let _ = arbos_state.l2_pricing_state.add_to_gas_pool(compute_gas_i64);
            }
        }
        
        state.poster_fee = U256::ZERO;
        state.poster_gas = 0;
    }

    fn nonrefundable_gas(&self, state: &ArbTxProcessorState) -> u64 {
        state.poster_gas
    }

    fn held_gas(&self, state: &ArbTxProcessorState) -> u64 {
        state.compute_hold_gas
    }
    
    fn scheduled_txes<D: Database>(
        &self,
        _state_db: &mut revm::database::State<D>,
        _state: &ArbTxProcessorState,
        logs: &[alloy_primitives::Log],
        _chain_id: u64,
        _block_timestamp: u64,
        _basefee: U256,
    ) -> Vec<Vec<u8>> {
        use arb_alloy_predeploys::ARB_RETRYABLE_TX;
        use alloy_primitives::keccak256;
        
        let redeem_scheduled_event_id = keccak256("RedeemScheduled(bytes32,bytes32,uint64,uint64,address,uint256,uint256)");
        
        let mut scheduled = Vec::new();
        
        for log in logs {
            let topics = log.topics();
            
            if log.address != ARB_RETRYABLE_TX || topics.is_empty() || topics[0] != redeem_scheduled_event_id {
                continue;
            }
            
            if topics.len() >= 3 {
                tracing::debug!("Found RedeemScheduled event for ticket {:?}", topics[1]);
            }
        }
        
        scheduled
    }
    
    fn l1_block_number<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
    ) -> Result<u64, ()> {
        if let Some(cached) = state.cached_l1_block_number {
            return Ok(cached);
        }
        
        if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
            if let Ok(block_num) = arbos_state.blockhashes.l1_block_number() {
                state.cached_l1_block_number = Some(block_num);
                return Ok(block_num);
            }
        }
        
        Ok(0)
    }
    
    fn l1_block_hash<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &mut ArbTxProcessorState,
        l1_block_number: u64,
    ) -> Result<B256, ()> {
        if let Some(cached) = state.cached_l1_block_hashes.get(&l1_block_number) {
            return Ok(*cached);
        }
        
        if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
            if let Ok(Some(hash)) = arbos_state.blockhashes.block_hash(l1_block_number) {
                state.cached_l1_block_hashes.insert(l1_block_number, hash);
                return Ok(hash);
            }
        }
        
        Ok(B256::ZERO)
    }
    
    fn drop_tip(&self, state: &ArbTxProcessorState) -> bool {
        state.arbos_version != 9 || state.delayed_inbox
    }
    
    fn get_paid_gas_price(&self, state: &ArbTxProcessorState, evm_gas_price: U256, basefee: U256) -> U256 {
        if state.arbos_version != 9 {
            basefee
        } else {
            evm_gas_price
        }
    }
    
    fn gas_price_op(&self, state: &ArbTxProcessorState, evm_gas_price: U256, basefee: U256) -> U256 {
        if state.arbos_version >= 3 {
            self.get_paid_gas_price(state, evm_gas_price, basefee)
        } else {
            evm_gas_price
        }
    }
    
    fn fill_receipt_info(&self, state: &ArbTxProcessorState) -> u64 {
        state.poster_gas
    }
    
    fn msg_is_non_mutating(&self, ctx: &ArbStartTxContext) -> bool {
        ctx.is_eth_call
    }
    
    fn is_calldata_pricing_increase_enabled<D: Database>(
        &self,
        state_db: &mut revm::database::State<D>,
        state: &ArbTxProcessorState,
    ) -> bool {
        if state.arbos_version < 40 {
            return false;
        }
        
        if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
            if let Ok(enabled) = arbos_state.features.is_increased_calldata_price_enabled() {
                return enabled;
            }
        }
        
        false
    }
    
    fn push_contract(&self, state: &mut ArbTxProcessorState, contract_addr: Address, is_delegate_or_callcode: bool) {
        state.contracts_stack.push(contract_addr);
        if !is_delegate_or_callcode {
            *state.programs_map.entry(contract_addr).or_insert(0) += 1;
        }
    }
    
    fn pop_contract(&self, state: &mut ArbTxProcessorState, is_delegate_or_callcode: bool) {
        if let Some(popped) = state.contracts_stack.pop() {
            if !is_delegate_or_callcode {
                if let Some(count) = state.programs_map.get_mut(&popped) {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }
    
    fn execute_wasm<D: Database>(
        &self,
        _state_db: &mut revm::database::State<D>,
        state: &ArbTxProcessorState,
        contract_addr: Address,
        _input: &[u8],
    ) -> Result<Vec<u8>, Vec<u8>> {
        let _reentrant = state.programs_map.get(&contract_addr).copied().unwrap_or(0) > 1;
        
        Err(b"WASM execution not yet implemented".to_vec())
    }
}

pub fn enforce_gas_limit<D: Database>(
    state_db: &mut revm::database::State<D>,
    state: &mut ArbTxProcessorState,
    gas_remaining: &mut u64,
    intrinsic_gas: u64,
    is_eth_call: bool,
) -> Result<(), ()> {
    if is_eth_call {
        return Ok(());
    }
    
    if let Ok(arbos_state) = ArbosState::open(state_db as *mut _) {
        let max = if state.arbos_version < 50 {
            arbos_state.l2_pricing_state.get_per_block_gas_limit().unwrap_or(32_000_000)
        } else {
            let mut max = arbos_state.l2_pricing_state.get_per_block_gas_limit().unwrap_or(32_000_000);
            max = max.saturating_sub(intrinsic_gas);
            max
        };
        
        if *gas_remaining > max {
            state.compute_hold_gas = *gas_remaining - max;
            *gas_remaining = max;
        }
    }
    
    Ok(())
}

pub struct CurrentRetryData {
    pub ticket_id: B256,
    pub from: Address,
    pub refund_to: Address,
    pub value: U256,
    pub gas_fee_cap: U256,
    pub max_refund: U256,
    pub submission_fee_refund: U256,
}

pub struct ArbTxProcessorState {
    pub poster_fee: U256,
    pub poster_gas: u64,
    pub compute_hold_gas: u64,
    pub delayed_inbox: bool,
    pub retryables: Option<*mut revm::database::State<()>>,
    pub network_fee_account: Address,
    pub infra_fee_account: Address,
    pub brotli_compression_level: u32,
    pub l1_base_fee: U256,
    pub arbos_version: u64,
    pub min_base_fee: U256,
    pub current_retry_data: Option<CurrentRetryData>,
    pub cached_l1_block_number: Option<u64>,
    pub cached_l1_block_hashes: std::collections::HashMap<u64, B256>,
    pub contracts_stack: Vec<Address>,
    pub programs_map: std::collections::HashMap<Address, u32>,
}

impl Default for ArbTxProcessorState {
    fn default() -> Self {
        let network_fee_account = Address::from([0u8; 20]);
        let infra_fee_account = Address::from([0u8; 20]);
        
        Self {
            poster_fee: U256::ZERO,
            poster_gas: 0,
            compute_hold_gas: 0,
            delayed_inbox: false,
            retryables: None,
            network_fee_account,
            infra_fee_account,
            brotli_compression_level: 0,
            l1_base_fee: U256::ZERO,
            arbos_version: 11,
            min_base_fee: U256::from(100_000_000u64),
            current_retry_data: None,
            cached_l1_block_number: None,
            cached_l1_block_hashes: std::collections::HashMap::new(),
            contracts_stack: Vec::new(),
            programs_map: std::collections::HashMap::new(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};

    struct DummyEvm;

    #[test]
    fn gas_charging_applies_poster_data_cost_with_padding() {
        let hooks = DefaultArbOsHooks::default();
        let mut state = ArbTxProcessorState::default();

        let basefee = U256::from(1_000u64);
        let calldata = vec![0u8; 100];
        let ctx = ArbGasChargingContext {
            intrinsic_gas: 21_000,
            calldata: calldata.clone(),
            basefee,
            is_executed_on_chain: true,
            skip_l1_charging: false,
        };

        let mut evm = DummyEvm;
        let (_tip, res) = hooks.gas_charging(&mut evm, &mut state, &ctx);
        assert!(res.is_ok());

        let units = arb_alloy_util::l1_pricing::L1PricingState::poster_units_from_brotli_len(calldata.len() as u64);
        let padded = arb_alloy_util::l1_pricing::L1PricingState::apply_estimation_padding(units);
        let expected_fee = U256::from(padded) * basefee;
        assert_eq!(state.poster_fee, expected_fee);

        let expected_gas: u64 = (expected_fee / basefee).try_into().unwrap();
        assert_eq!(state.poster_gas, expected_gas);
    }

    #[test]
    fn end_tx_accumulates_hold_gas_and_resets_poster_fields() {
        let hooks = DefaultArbOsHooks::default();
        let mut state = ArbTxProcessorState::default();
        state.poster_fee = U256::from(12345u64);
        state.poster_gas = 6789u64;
        let before_hold = state.compute_hold_gas;

        let mut evm = DummyEvm;
        let ctx = ArbEndTxContext {
            success: true,
            gas_left: 0,
            gas_limit: 1_000_000,
            basefee: U256::from(1_000u64),
        };
        hooks.end_tx(&mut evm, &mut state, &ctx);

        assert_eq!(state.compute_hold_gas, before_hold.saturating_add(6789u64));
        assert_eq!(state.poster_fee, U256::ZERO);
        assert_eq!(state.poster_gas, 0);
    }

    #[test]
    fn start_tx_sets_delayed_inbox_flag_from_coinbase() {
        let hooks = DefaultArbOsHooks::default();
        let mut state = ArbTxProcessorState::default();
        let mut evm = DummyEvm;
        let ctx = ArbStartTxContext {
            sender: Address::ZERO,
            nonce: 0,
            l1_base_fee: U256::ZERO,
            calldata_len: 0,
            coinbase: Address::from([1u8; 20]),
            executed_on_chain: true,
            is_eth_call: false,
            tx_type: 0x02,
            to: Some(Address::ZERO),
            value: U256::ZERO,
            gas_limit: 100000,
            basefee: U256::from(1000u64),
            ticket_id: None,
            refund_to: None,
            gas_fee_cap: None,
            max_refund: None,
            submission_fee_refund: None,
            tx_hash: B256::ZERO,
            deposit_value: None,
            retry_value: None,
            retry_to: None,
            retry_data: None,
            beneficiary: None,
            max_submission_fee: None,
            fee_refund_addr: None,
            block_timestamp: 0,
        };
        hooks.start_tx(&mut evm, &mut state, &ctx);
        assert!(state.delayed_inbox);
    }
}
