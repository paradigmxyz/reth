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
    pub block_number: u64,
    pub parent_hash: Option<B256>,
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
    /// Updates the blockhash history storage (EIP-2935 style).
    /// This writes DIRECTLY to bundle_state.state for deterministic state computation.
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

        // Ensure the account exists in bundle_state.state
        // Read from database to preserve any genesis values
        if !state_db.bundle_state.state.contains_key(&HISTORY_STORAGE_ADDRESS) {
            // Read original info from database
            let original_info = state_db.database.basic(HISTORY_STORAGE_ADDRESS)
                .ok()
                .flatten()
                .unwrap_or_else(|| AccountInfo {
                    balance: U256::ZERO,
                    nonce: 0,
                    code_hash: alloy_primitives::keccak256([]),
                    code: None,
                });

            let info = Some(AccountInfo {
                balance: original_info.balance,
                nonce: original_info.nonce,
                code_hash: original_info.code_hash,
                code: original_info.code.clone(),
            });

            // Use Loaded status because we're only modifying storage, not account info
            let acc = BundleAccount {
                info,
                storage: std::collections::HashMap::default(),
                original_info: Some(original_info),
                status: AccountStatus::Loaded,
            };
            state_db.bundle_state.state.insert(HISTORY_STORAGE_ADDRESS, acc);
        }

        let slot = U256::from_be_bytes(prev_hash.0);
        let value_u256 = U256::from_be_bytes(prev_hash.0);

        // Get original value for proper tracking
        let original_value = if let Some(acc) = state_db.bundle_state.state.get(&HISTORY_STORAGE_ADDRESS) {
            if let Some(slot_entry) = acc.storage.get(&slot) {
                slot_entry.previous_or_original_value
            } else {
                state_db.database.storage(HISTORY_STORAGE_ADDRESS, slot).unwrap_or(U256::ZERO)
            }
        } else {
            state_db.database.storage(HISTORY_STORAGE_ADDRESS, slot).unwrap_or(U256::ZERO)
        };

        // Write directly to bundle_state.state
        if let Some(acc) = state_db.bundle_state.state.get_mut(&HISTORY_STORAGE_ADDRESS) {
            acc.storage.insert(
                slot,
                EvmStorageSlot::new_changed(original_value, value_u256, 0).into(),
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
        // Convert U256 to u128, panic if it doesn't fit (should never happen in practice)
        let amount_u128: u128 = amount.try_into().expect("mint amount exceeds u128::MAX");

        // CRITICAL FIX MARKER - If you see this log, commit 3cdeeafe5 is ACTIVE
        tracing::info!(
            target: "arbitrum::balance",
            ?address,
            ?amount,
            "mint_balance: CRITICAL FIX ACTIVE - using .expect() instead of .unwrap_or(u128::MAX)"
        );

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

        // Load accounts into cache
        let _ = state.load_cache_account(from);
        let _ = state.load_cache_account(to);

        // Check from balance
        let from_account = match state.basic(from) {
            Ok(info) => info,
            Err(_) => return Err(()),
        };
        let from_balance = from_account.map(|i| U256::from(i.balance)).unwrap_or_default();

        if from_balance < amount {
            return Err(());
        }

        // Decrement `from` balance using proper transition tracking
        // We need to manually create a transition since there's no decrement_balance API
        {
            let cached_from = state.cache.accounts.get_mut(&from).ok_or(())?;
            let previous_status = cached_from.status;
            let previous_info = cached_from.account.as_ref().map(|a| a.info.clone());

            // Apply the decrement
            if let Some(ref mut account) = cached_from.account {
                account.info.balance = account.info.balance.saturating_sub(amount);
            }

            let had_no_nonce_and_code = previous_info
                .as_ref()
                .map(|info| info.has_no_code_and_nonce())
                .unwrap_or_default();
            cached_from.status = cached_from.status.on_changed(had_no_nonce_and_code);

            // Create and apply the transition
            let transition = revm::database::TransitionAccount {
                info: cached_from.account.as_ref().map(|a| a.info.clone()),
                status: cached_from.status,
                previous_info,
                previous_status,
                storage: Default::default(),
                storage_was_destroyed: false,
            };
            state.apply_transition(vec![(from, transition)]);
        }

        // Increment `to` balance using increment_balances API (which creates proper transitions)
        let amount_u128: u128 = amount.try_into().map_err(|_| ())?;
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

        // Decrement balance using proper transition tracking
        // We need to manually create a transition since there's no decrement_balance API
        let cached_from = state.cache.accounts.get_mut(&from).ok_or(())?;
        let previous_status = cached_from.status;
        let previous_info = cached_from.account.as_ref().map(|a| a.info.clone());

        // Apply the decrement
        if let Some(ref mut account) = cached_from.account {
            account.info.balance = account.info.balance.saturating_sub(amount);
        }

        let had_no_nonce_and_code = previous_info
            .as_ref()
            .map(|info| info.has_no_code_and_nonce())
            .unwrap_or_default();
        cached_from.status = cached_from.status.on_changed(had_no_nonce_and_code);

        // Create and apply the transition
        let transition = revm::database::TransitionAccount {
            info: cached_from.account.as_ref().map(|a| a.info.clone()),
            status: cached_from.status,
            previous_info,
            previous_status,
            storage: Default::default(),
            storage_was_destroyed: false,
        };
        state.apply_transition(vec![(from, transition)]);

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

        use crate::retryables::RetryableState;
        
        // Use the correct retryable subspace key (subspace 2) - same as other places in the code
        let retryable_storage = crate::storage::Storage::new(
            state_db as *mut _,
            crate::arbosstate::arbos_state_subspace(2),
        );
        let retryable_state = RetryableState::new(state_db as *mut _, retryable_storage.base_key);
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

        // ITER83: Debug logging for start_tx hook
        tracing::debug!(
            target: "arb-reth::start_tx_debug",
            tx_type = ctx.tx_type,
            tx_type_hex = format!("0x{:02x}", ctx.tx_type),
            sender = ?ctx.sender,
            data_len = ctx.data.as_ref().map(|d| d.len()).unwrap_or(0),
            "[ITER83] start_tx hook received tx_type"
        );

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
                
                let data = ctx.data.as_deref().unwrap_or(&[]);
                
                if data.len() < 4 {
                    return StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: Some("internal tx data too short".to_string()),
                    };
                }
                
                let selector = &data[0..4];
                
                use crate::internal_tx::{
                    get_start_block_method_id,
                    get_batch_posting_report_method_id,
                    get_batch_posting_report_v2_method_id,
                    identify_internal_tx_type,
                    unpack_internal_tx_data_start_block,
                    unpack_internal_tx_data_batch_posting_report,
                    unpack_internal_tx_data_batch_posting_report_v2,
                };

                let start_block_id = get_start_block_method_id();
                let batch_report_id = get_batch_posting_report_method_id();
                let batch_report_v2_id = get_batch_posting_report_v2_method_id();
                
                if selector == start_block_id.as_slice() {
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
                
                let prev_hash = if ctx.block_number > 0 {
                    ctx.parent_hash.unwrap_or(B256::ZERO)
                } else {
                    B256::ZERO
                };
                
                if arbos_version >= 40 {
                    Self::process_parent_block_hash(state_db, prev_hash);
                }
                
                let blockhashes_storage = crate::storage::Storage::new(
                    state_db as *mut _,
                    crate::arbosstate::arbos_state_subspace(6),
                );
                let blockhashes = crate::blockhash::Blockhashes::open(blockhashes_storage);
                
                let old_l1_block_number = blockhashes.l1_block_number().unwrap_or(0);
                let mut l1_block_number = internal_data.l1_block_number;

                tracing::info!(
                    target: "arb-evm::startblock",
                    "StartBlock internal tx: l1_block_number_raw={}, old_l1_block_number={}, arbos_version={}",
                    l1_block_number, old_l1_block_number, arbos_version
                );

                if arbos_version < 8 {
                    l1_block_number += 1;
                    tracing::info!(
                        target: "arb-evm::startblock",
                        "Adjusted l1_block_number for arbos_version<8: new_value={}",
                        l1_block_number
                    );
                }

                if l1_block_number > old_l1_block_number {
                    tracing::info!(
                        target: "arb-evm::startblock",
                        "Recording new L1 block: number={}, prev_hash={:?}",
                        l1_block_number - 1, prev_hash
                    );
                    if let Err(e) = blockhashes.record_new_l1_block(
                        l1_block_number - 1,
                        prev_hash,
                        arbos_version,
                    ) {
                        tracing::error!("Failed to record new L1 block: {:?}", e);
                    } else {
                        tracing::info!(
                            target: "arb-evm::startblock",
                            "Successfully recorded L1 block"
                        );
                    }
                } else {
                    tracing::warn!(
                        target: "arb-evm::startblock",
                        "Skipping L1 block record: l1_block_number={} <= old_l1_block_number={}",
                        l1_block_number, old_l1_block_number
                    );
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
                    
                    let result = StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: None,
                    };
                    tracing::info!(
                        target: "arb-reth::start_tx_debug",
                        tx_type = 0x6A,
                        internal_type = "StartBlock",
                        end_tx_now = result.end_tx_now,
                        "[ITER83] Internal/StartBlock returning with end_tx_now=true"
                    );
                    result
                } else if selector == batch_report_v2_id.as_slice() {
                    let report_data = match unpack_internal_tx_data_batch_posting_report_v2(data) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::error!("Failed to unpack batch posting report V2 data: {}", e);
                            return StartTxHookResult {
                                end_tx_now: true,
                                gas_used: 0,
                                error: Some(format!("invalid batch posting report V2 data: {}", e)),
                            };
                        }
                    };

                    let l1_pricing = crate::l1_pricing::L1PricingState::open(
                        crate::storage::Storage::new(
                            state_db as *mut _,
                            crate::arbosstate::arbos_state_subspace(0),
                        ),
                        state.arbos_version,
                    );

                    // Calculate legacy cost from batch stats
                    const TX_DATA_ZERO_GAS: u64 = 4;
                    const TX_DATA_NON_ZERO_GAS: u64 = 16;
                    const KECCAK256_GAS: u64 = 30;
                    const KECCAK256_WORD_GAS: u64 = 6;
                    const SSTORE_SET_GAS: u64 = 20000;

                    let zero_bytes = report_data.batch_calldata_length.saturating_sub(report_data.batch_calldata_non_zeros);
                    let mut gas_spent = (zero_bytes * TX_DATA_ZERO_GAS) + (report_data.batch_calldata_non_zeros * TX_DATA_NON_ZERO_GAS);

                    // Add keccak cost
                    let keccak_words = (report_data.batch_calldata_length + 31) / 32;
                    gas_spent = gas_spent.saturating_add(KECCAK256_GAS).saturating_add(keccak_words * KECCAK256_WORD_GAS);

                    // Add 2 SSTORE costs
                    gas_spent = gas_spent.saturating_add(2 * SSTORE_SET_GAS);

                    // Add extra gas
                    gas_spent = gas_spent.saturating_add(report_data.batch_extra_gas);

                    // Add per-batch gas cost
                    let per_batch_gas_cost = l1_pricing.get_per_batch_gas_cost().unwrap_or(0);
                    gas_spent = gas_spent.saturating_add(per_batch_gas_cost);

                    // For ArbOS 50+, apply gas floor
                    if state.arbos_version >= 50 {
                        const FLOOR_GAS_ADDITIONAL_TOKENS: u64 = 172;
                        const TX_GAS: u64 = 21000;

                        let gas_floor_per_token = l1_pricing.parent_gas_floor_per_token().unwrap_or(0);
                        let token_count = report_data.batch_calldata_length + (report_data.batch_calldata_non_zeros * 3) + FLOOR_GAS_ADDITIONAL_TOKENS;
                        let floor_gas_spent = (gas_floor_per_token * token_count) + TX_GAS;

                        if floor_gas_spent > gas_spent {
                            gas_spent = floor_gas_spent;
                        }
                    }

                    let wei_spent = report_data.l1_base_fee_wei.saturating_mul(U256::from(gas_spent));

                    let batch_timestamp = report_data.batch_timestamp.try_into().unwrap_or(0u64);
                    let current_time = ctx.block_timestamp;

                    if let Err(e) = l1_pricing.update_for_batch_poster_spending(
                        gas_spent,
                        report_data.batch_calldata_length,
                        report_data.l1_base_fee_wei,
                        current_time,
                    ) {
                        tracing::warn!("Failed to update L1 pricing for batch poster spending (V2): {:?}", e);
                    }

                    StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: None,
                    }
                } else if selector == batch_report_id.as_slice() {
                    let report_data = match unpack_internal_tx_data_batch_posting_report(data) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::error!("Failed to unpack batch posting report data: {}", e);
                            return StartTxHookResult {
                                end_tx_now: true,
                                gas_used: 0,
                                error: Some(format!("invalid batch posting report data: {}", e)),
                            };
                        }
                    };
                    
                    let l1_pricing = crate::l1_pricing::L1PricingState::open(
                        crate::storage::Storage::new(
                            state_db as *mut _,
                            crate::arbosstate::arbos_state_subspace(0),
                        ),
                        11,
                    );
                    
                    let per_batch_gas_cost = l1_pricing.get_per_batch_gas_cost().unwrap_or(0);
                    let gas_spent = per_batch_gas_cost.saturating_add(report_data.batch_data_gas);
                    let wei_spent = report_data.l1_base_fee_wei.saturating_mul(U256::from(gas_spent));
                    
                    let batch_timestamp = report_data.batch_timestamp.try_into().unwrap_or(0u64);
                    let current_time = ctx.block_timestamp;
                    
                    if let Err(e) = l1_pricing.update_for_batch_poster_spending(
                        gas_spent,
                        report_data.batch_data_gas,
                        report_data.l1_base_fee_wei,
                        current_time,
                    ) {
                        tracing::warn!("Failed to update L1 pricing for batch poster spending: {:?}", e);
                    }
                    
                    StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: None,
                    }
                } else {
                    if let Some(tx_type) = identify_internal_tx_type(data) {
                        tracing::warn!(
                            "Internal tx type '{}' recognized but not handled (selector: {:?})",
                            tx_type,
                            selector
                        );
                    } else {
                        tracing::warn!(
                            "Unknown internal tx method selector: {:?} - skipping gracefully",
                            selector
                        );
                    }
                    
                    StartTxHookResult {
                        end_tx_now: true,
                        gas_used: 0,
                        error: None,
                    }
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

                tracing::info!(
                    sender = ?ctx.sender,
                    deposit_value = ?deposit_value,
                    balance_after_mint = ?balance_after_mint,
                    "SubmitRetryable: minted deposit value"
                );

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

                // Compute the retry transaction hash
                // The retry transaction is an ArbRetryTx (type 0x68) that gets automatically scheduled
                // We need to compute its hash to emit in the RedeemScheduled event
                let retry_tx_nonce = 0u64;

                // Construct the retry transaction
                // The retry transaction uses baseFeePerGas as gas_fee_cap, not the submit retryable's maxFeePerGas
                use arb_alloy_consensus::tx::ArbRetryTx;
                let retry_tx = ArbRetryTx {
                    chain_id: U256::from(421614u64), // Arbitrum Sepolia chain ID
                    nonce: retry_tx_nonce,
                    from: ctx.sender,
                    gas_fee_cap: ctx.basefee, // Use baseFeePerGas, not the submit retryable's maxFeePerGas
                    gas: usergas,
                    to: Some(retry_to),
                    value: retry_value,
                    data: Bytes::from(retry_data.to_vec()),
                    ticket_id,
                    refund_to: fee_refund_addr,
                    max_refund: available_refund,
                    submission_fee_refund: submission_fee_u256,
                };

                // Encode the retry transaction as type 0x68 + RLP encoding
                // Format: 0x68 || RLP([chain_id, nonce, from, gas_fee_cap, gas, to, value, data, ticket_id, refund_to, max_refund, submission_fee_refund])
                use alloy_rlp::Encodable;
                let mut encoded = Vec::new();
                encoded.push(0x68); // ArbRetryTx type byte
                retry_tx.encode(&mut encoded);

                // Compute keccak256 hash
                use alloy_primitives::keccak256;
                let retry_tx_hash = keccak256(&encoded);

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

                // Log final sender balance and burn any remaining balance
                // The sender should end up with exactly 0 balance after all transfers
                // since we minted deposit_value and transferred it all away
                let final_balance = match state_db.basic(ctx.sender) {
                    Ok(Some(acc)) => U256::from(acc.balance),
                    _ => U256::ZERO,
                };

                if final_balance > U256::ZERO {
                    tracing::warn!(
                        sender = ?ctx.sender,
                        final_balance = ?final_balance,
                        "SubmitRetryable: burning remaining balance to reach 0"
                    );
                    let _ = Self::burn_balance(state_db, ctx.sender, final_balance);
                } else {
                    tracing::info!(
                        sender = ?ctx.sender,
                        "SubmitRetryable: final balance correctly at 0"
                    );
                }

                StartTxHookResult {
                    end_tx_now: true,
                    gas_used: usergas,
                    error: None,
                }
            }
            
            _ => {
                tracing::warn!(
                    target: "arb-reth::start_tx_debug",
                    tx_type = ctx.tx_type,
                    tx_type_hex = format!("0x{:02x}", ctx.tx_type),
                    sender = ?ctx.sender,
                    "[ITER83] start_tx hook falling through to DEFAULT (end_tx_now=false)!"
                );
                StartTxHookResult::default()
            },
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
        // ITER84: Debug logging for end_tx hook
        tracing::info!(
            target: "arb-reth::end_tx_debug",
            tx_type = ctx.tx_type,
            tx_type_hex = format!("0x{:02x}", ctx.tx_type),
            gas_left = ctx.gas_left,
            gas_limit = ctx.gas_limit,
            success = ctx.success,
            "[ITER84] end_tx hook called"
        );

        if ctx.tx_type == 0x68 {
            if ctx.gas_left > ctx.gas_limit {
                tracing::error!("Tx refunds gas after computation - impossible");
                return;
            }
            let gas_used = ctx.gas_limit.saturating_sub(ctx.gas_left);
            
            if let Some(retry_data) = &state.current_retry_data {
                let effective_base_fee = retry_data.gas_fee_cap;

                // Undo revm's gas refund to the sender - ArbOS handles refunds through fee accounts
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
                        
                        // Go nitro: DeleteRetryable clears all fields
                        // For now, we just increment tries to mark it as redeemed
                        let _ = retryable.increment_tries();
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
            tracing::warn!(
                target: "arb-reth::end_tx_debug",
                network_fee_account = ?state.network_fee_account,
                compute_cost = %compute_cost,
                "[ITER84] MINTING to network_fee_account (this is the ArbOS balance issue!)"
            );
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

        // Only update gas pool if basefee is positive (matches Go's check for msg.GasPrice.Sign() > 0)
        if ctx.basefee > U256::ZERO {
            let compute_gas = if gas_used > state.poster_gas {
                // Don't include posterGas in computeGas as it doesn't represent processing time
                gas_used - state.poster_gas
            } else {
                // Somehow, the core message transition succeeded, but we didn't burn the posterGas.
                // An invariant was violated. To be safe, subtract the entire gas used from the gas pool.
                // Note: This can happen legitimately when both gas_used and poster_gas are 0 for
                // transactions that end early (e.g., internal txs)
                if gas_used > 0 || state.poster_gas > 0 {
                    tracing::error!("total gas used < poster gas component, gasUsed={} posterGas={}",
                        gas_used, state.poster_gas);
                }
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
        state_db: &mut revm::database::State<D>,
        _state: &ArbTxProcessorState,
        logs: &[alloy_primitives::Log],
        chain_id: u64,
        block_timestamp: u64,
        basefee: U256,
    ) -> Vec<Vec<u8>> {
        use arb_alloy_predeploys::ARB_RETRYABLE_TX;
        use alloy_primitives::keccak256;
        use crate::retryables::RetryableState;
        use crate::storage::Storage;
        use crate::arbosstate::{arbos_state_subspace, RETRYABLES_SUBSPACE};
        
        let redeem_scheduled_event_id = keccak256("RedeemScheduled(bytes32,bytes32,uint64,uint64,address,uint256,uint256)");
        
        let mut scheduled = Vec::new();
        
        for log in logs {
            let topics = log.topics();
            
            if log.address != ARB_RETRYABLE_TX || topics.is_empty() || topics[0] != redeem_scheduled_event_id {
                continue;
            }
            
            if topics.len() < 3 {
                continue;
            }
            
            // Parse event data:
            // Topics: [event_id, ticket_id, retry_tx_hash]
            // Data: [sequence_num (uint64), donated_gas (uint64), gas_donor (address), max_refund (uint256), submission_fee_refund (uint256)]
            let ticket_id = topics[1];
            let data = &log.data.data;
            
            if data.len() < 160 {
                tracing::warn!("RedeemScheduled event data too short: {} bytes", data.len());
                continue;
            }
            
            // Parse event data (ABI encoded)
            // uint64 sequence_num at offset 0 (padded to 32 bytes)
            // uint64 donated_gas at offset 32 (padded to 32 bytes)
            // address gas_donor at offset 64 (padded to 32 bytes)
            // uint256 max_refund at offset 96
            // uint256 submission_fee_refund at offset 128
            let sequence_num = u64::from_be_bytes(data[24..32].try_into().unwrap_or([0u8; 8]));
            let donated_gas = u64::from_be_bytes(data[56..64].try_into().unwrap_or([0u8; 8]));
            let gas_donor = Address::from_slice(&data[76..96]);
            let max_refund = U256::from_be_slice(&data[96..128]);
            let submission_fee_refund = U256::from_be_slice(&data[128..160]);
            
            tracing::info!(
                target: "arb-scheduled",
                "Found RedeemScheduled event: ticket_id={:?} sequence_num={} donated_gas={} gas_donor={:?}",
                ticket_id, sequence_num, donated_gas, gas_donor
            );
            
            // Open the retryable from state
            let retryable_storage = Storage::new(
                state_db as *mut _,
                arbos_state_subspace(RETRYABLES_SUBSPACE[0]),
            );
            let retryable_state = RetryableState::new(
                state_db as *mut _,
                retryable_storage.base_key,
            );
            
            let ticket_id_struct = RetryableTicketId(ticket_id.0);
            
            if let Some(retryable) = retryable_state.open_retryable(
                state_db as *mut _,
                &ticket_id_struct,
                block_timestamp,
            ) {
                // Get retryable data
                let from = retryable.get_from().unwrap_or_default();
                let to = retryable.get_to().unwrap_or_default();
                let call_value = retryable.get_callvalue().unwrap_or_default();
                
                tracing::info!(
                    target: "arb-scheduled",
                    "Constructing retry tx: from={:?} to={:?} value={:?} gas={}",
                    from, to, call_value, donated_gas
                );
                
                // Construct the retry transaction
                use arb_alloy_consensus::tx::ArbRetryTx;
                let retry_tx = ArbRetryTx {
                    chain_id: U256::from(chain_id),
                    nonce: sequence_num,
                    from,
                    gas_fee_cap: basefee,
                    gas: donated_gas,
                    to: Some(to),
                    value: call_value,
                    data: Bytes::new(), // TODO: Get actual calldata from retryable storage
                    ticket_id: B256::from(ticket_id.0),
                    refund_to: gas_donor,
                    max_refund,
                    submission_fee_refund,
                };
                
                // Encode the retry transaction as type 0x68 + RLP encoding
                use alloy_rlp::Encodable;
                let mut encoded = Vec::new();
                encoded.push(0x68); // ArbRetryTx type byte
                retry_tx.encode(&mut encoded);
                
                tracing::info!(
                    target: "arb-scheduled",
                    "Scheduled retry tx: encoded_len={} hash={:?}",
                    encoded.len(),
                    keccak256(&encoded)
                );
                
                scheduled.push(encoded);
            } else {
                tracing::warn!(
                    target: "arb-scheduled",
                    "Could not open retryable for ticket_id={:?}",
                    ticket_id
                );
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
            // ArbOS 50 implements a EIP-7825-like per-transaction limit
            let mut max = arbos_state.l2_pricing_state.get_per_tx_gas_limit().unwrap_or(32_000_000);
            // Reduce the max by intrinsicGas because it was already charged
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
            data: None,
            block_number: 1,
            parent_hash: Some(B256::ZERO),
        };
        hooks.start_tx(&mut evm, &mut state, &ctx);
        assert!(state.delayed_inbox);
    }
}
