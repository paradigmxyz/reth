#![allow(unused)]

use alloy_primitives::{Address, U256};
use reth_evm::Evm;

pub struct ArbStartTxContext {
    pub sender: Address,
    pub nonce: u64,
    pub l1_base_fee: U256,
    pub calldata_len: usize,
}

pub struct ArbGasChargingContext {
    pub intrinsic_gas: u64,
    pub calldata: Vec<u8>,
}

pub struct ArbEndTxContext {
    pub success: bool,
    pub gas_left: u64,
    pub gas_limit: u64,
}

pub trait ArbOsHooks {
    fn start_tx<E: Evm>(&self, evm: &mut E, ctx: &ArbStartTxContext);
    fn gas_charging<E: Evm>(&self, evm: &mut E, ctx: &ArbGasChargingContext) -> (Address, Option<u64>, Option<U256>);
    fn end_tx<E: Evm>(&self, evm: &mut E, ctx: &ArbEndTxContext);
}

#[derive(Default, Clone)]
pub struct DefaultArbOsHooks;

impl ArbOsHooks for DefaultArbOsHooks {
    fn start_tx<E: Evm>(&self, _evm: &mut E, _ctx: &ArbStartTxContext) {}

    fn gas_charging<E: Evm>(&self, _evm: &mut E, _ctx: &ArbGasChargingContext) -> (Address, Option<u64>, Option<U256>) {
        (Address::ZERO, None, None)
    }

    fn end_tx<E: Evm>(&self, _evm: &mut E, _ctx: &ArbEndTxContext) {}
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
