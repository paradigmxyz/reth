use alloy_primitives::{Address, U256, B256};
use arb_alloy_util::l1_pricing::L1PricingState;
use crate::retryables::{Retryables, DefaultRetryables, RetryableCreateParams, RetryableAction, RetryableTicketId};
use reth_arbitrum_primitives::{ArbTxType, ArbTransactionSigned, ArbTypedTransaction};
use alloy_consensus::Transaction as AlloyCoinbaseTransaction;
use revm::Database;

pub struct ArbStartTxContext {
    pub sender: Address,
    pub nonce: u64,
    pub l1_base_fee: U256,
    pub calldata_len: usize,
    pub coinbase: Address,
    pub executed_on_chain: bool,
    pub is_eth_call: bool,
}

pub struct ArbGasChargingContext {
    pub intrinsic_gas: u64,
    pub calldata: Vec<u8>,
    pub basefee: U256,
    pub is_executed_on_chain: bool,
    pub skip_l1_charging: bool,
}

pub struct ArbEndTxContext {
    pub success: bool,
    pub gas_left: u64,
    pub gas_limit: u64,
    pub basefee: U256,
}

pub trait ArbOsHooks {
    fn start_tx<E>(&self, evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbStartTxContext);
    fn gas_charging<E>(
        &self,
        evm: &mut E,
        state: &mut ArbTxProcessorState,
        ctx: &ArbGasChargingContext,
    ) -> (Address, Result<(), ()>);
    fn end_tx<E>(&self, evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbEndTxContext);
    fn nonrefundable_gas(&self, state: &ArbTxProcessorState) -> u64;
    fn held_gas(&self, state: &ArbTxProcessorState) -> u64;
}

#[derive(Default, Clone)]
pub struct DefaultArbOsHooks;

impl DefaultArbOsHooks {
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
        let amount_u128: u128 = amount.try_into().unwrap_or(u128::MAX);
        let _ = state.increment_balances(core::iter::once((address, amount_u128)));
    }

    fn transfer_balance<D>(
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

    fn take_funds(available: &mut U256, amount: U256) -> U256 {
        let taken = (*available).min(amount);
        *available = available.saturating_sub(taken);
        taken
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
        let retry_value_taken = Self::take_funds(&mut available_refund, retry_value);

        Self::mint_balance(state_db, from, deposit_value);

        let after_mint_account = match state_db.basic(from) {
            Ok(info) => info,
            Err(_) => return Err(()),
        };
        let balance_after_mint = after_mint_account.map(|i| U256::from(i.balance)).unwrap_or_default();

        if balance_after_mint < max_submission_fee {
            return Err(());
        }

        let submission_fee = U256::from(arb_alloy_util::retryables::retryable_submission_fee(
            retry_data.len(),
            l1_base_fee.try_into().unwrap_or(u128::MAX),
        ));

        if max_submission_fee < submission_fee {
            return Err(());
        }

        Self::transfer_balance(state_db, from, network_fee_account, submission_fee)?;
        let withheld_submission_fee = Self::take_funds(&mut available_refund, submission_fee);

        let submission_fee_refund = Self::take_funds(
            &mut available_refund,
            max_submission_fee.saturating_sub(submission_fee),
        );
        let _ = Self::transfer_balance(state_db, from, fee_refund_addr, submission_fee_refund);

        if let Err(_) = Self::transfer_balance(state_db, from, escrow, retry_value) {
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

        let action = state.retryables.create_retryable(params);

        Ok(())
    }
}

impl ArbOsHooks for DefaultArbOsHooks {
    fn start_tx<E>(&self, _evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbStartTxContext) {
        state.delayed_inbox = ctx.coinbase != Address::ZERO;
    }

    fn gas_charging<E>(
        &self,
        _evm: &mut E,
        state: &mut ArbTxProcessorState,
        ctx: &ArbGasChargingContext,
    ) -> (Address, Result<(), ()>) {
        let tip_recipient = Address::ZERO;
        if !ctx.skip_l1_charging && !ctx.basefee.is_zero() {
            let units = L1PricingState::poster_units_from_brotli_len(ctx.calldata.len() as u64);
            let padded_units = L1PricingState::apply_estimation_padding(units);
            let l1_base_fee_wei: u128 = ctx.basefee.try_into().unwrap_or_default();
            let pricing = L1PricingState { l1_base_fee_wei };
            let poster_cost_u128 = pricing.poster_data_cost_from_units(padded_units);
            let poster_cost = U256::from(poster_cost_u128);
            let poster_gas = Self::get_poster_gas(ctx.basefee, poster_cost);
            state.poster_gas = poster_gas;
            state.poster_fee = poster_cost;
        }
        (tip_recipient, Ok(()))
    }

    fn end_tx<E>(&self, _evm: &mut E, state: &mut ArbTxProcessorState, _ctx: &ArbEndTxContext) {
        state.compute_hold_gas = state.compute_hold_gas.saturating_add(state.poster_gas);
        state.poster_fee = U256::ZERO;
        state.poster_gas = 0;
    }

    fn nonrefundable_gas(&self, state: &ArbTxProcessorState) -> u64 {
        state.poster_gas
    }

    fn held_gas(&self, state: &ArbTxProcessorState) -> u64 {
        state.compute_hold_gas
    }
}

pub struct ArbTxProcessorState {
    pub poster_fee: U256,
    pub poster_gas: u64,
    pub compute_hold_gas: u64,
    pub delayed_inbox: bool,
    pub retryables: DefaultRetryables,
    pub network_fee_account: Address,
}

impl Default for ArbTxProcessorState {
    fn default() -> Self {
        let network_fee_account = Address::from([0u8; 20]);
        
        Self {
            poster_fee: U256::ZERO,
            poster_gas: 0,
            compute_hold_gas: 0,
            delayed_inbox: false,
            retryables: DefaultRetryables::default(),
            network_fee_account,
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
        };
        hooks.start_tx(&mut evm, &mut state, &ctx);
        assert!(state.delayed_inbox);
    }
}
