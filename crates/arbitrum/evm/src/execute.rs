#![allow(unused)]

use alloy_primitives::{Address, U256};
use reth_evm::Evm;
use arb_alloy_util::l1_pricing::L1PricingState;

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
    fn start_tx<E: Evm>(&self, evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbStartTxContext);
    fn gas_charging<E: Evm>(
        &self,
        evm: &mut E,
        state: &mut ArbTxProcessorState,
        ctx: &ArbGasChargingContext,
    ) -> (Address, Result<(), ()>);
    fn end_tx<E: Evm>(&self, evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbEndTxContext);
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
}

impl ArbOsHooks for DefaultArbOsHooks {
    fn start_tx<E: Evm>(&self, _evm: &mut E, state: &mut ArbTxProcessorState, ctx: &ArbStartTxContext) {
        state.delayed_inbox = ctx.coinbase != Address::ZERO;
    }

    fn gas_charging<E: Evm>(
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

    fn end_tx<E: Evm>(&self, _evm: &mut E, state: &mut ArbTxProcessorState, _ctx: &ArbEndTxContext) {
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
}

impl Default for ArbTxProcessorState {
    fn default() -> Self {
        Self {
            poster_fee: U256::ZERO,
            poster_gas: 0,
            compute_hold_gas: 0,
            delayed_inbox: false,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};

    struct DummyEvm;
    impl Evm for DummyEvm {}

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
