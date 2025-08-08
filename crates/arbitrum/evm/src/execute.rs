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
}

pub trait ArbOsHooks {
    fn start_tx<E: Evm>(&self, evm: &mut E, ctx: &ArbStartTxContext);
    fn gas_charging<E: Evm>(&self, evm: &mut E, ctx: &ArbGasChargingContext);
    fn end_tx<E: Evm>(&self, evm: &mut E, ctx: &ArbEndTxContext);
}

#[derive(Default, Clone)]
pub struct DefaultArbOsHooks;

impl ArbOsHooks for DefaultArbOsHooks {
    fn start_tx<E: Evm>(&self, _evm: &mut E, _ctx: &ArbStartTxContext) {}
    fn gas_charging<E: Evm>(&self, _evm: &mut E, _ctx: &ArbGasChargingContext) {}
    fn end_tx<E: Evm>(&self, _evm: &mut E, _ctx: &ArbEndTxContext) {}
}
#![allow(unused)]
