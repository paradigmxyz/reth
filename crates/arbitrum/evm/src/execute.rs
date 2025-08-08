#![allow(unused)]

use alloy_primitives::{Address, U256};
use reth_evm::Evm;

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
            let calldata_len = ctx.calldata.len() as u128;
            let per_byte = U256::from(6u64);
            let overhead = U256::from(1400u64);
            let fee_units = overhead + per_byte * U256::from(calldata_len);
            let poster_cost = fee_units.saturating_mul(ctx.basefee);
            let poster_gas = Self::get_poster_gas(ctx.basefee, poster_cost);
            state.poster_gas = poster_gas;
            state.poster_fee = poster_cost;
        }
        (tip_recipient, Ok(()))
    }

    fn end_tx<E: Evm>(&self, _evm: &mut E, state: &mut ArbTxProcessorState, _ctx: &ArbEndTxContext) {
        let _ = state.poster_fee;
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
    fn gas_charging_applies_retryable_submission_fee_math() {
        let hooks = DefaultArbOsHooks::default();
        let mut state = ArbTxProcessorState::default();

        let basefee = U256::from(1_000u64);
        let calldata = vec![0u8; 100]; // 100 bytes
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

        let expected_units = U256::from(1400u64) + U256::from(6u64) * U256::from(calldata.len() as u64);
        let expected_fee = expected_units * basefee;
        assert_eq!(state.poster_fee, expected_fee);

        let expected_gas: u64 = (expected_fee / basefee).try_into().unwrap();
        assert_eq!(state.poster_gas, expected_gas);
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
