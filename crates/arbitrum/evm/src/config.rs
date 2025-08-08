#![allow(unused)]
use alloc::sync::Arc;
use alloy_consensus::eip4895::Withdrawals;
use alloy_primitives::{Address, B256, Bytes, U256};

#[derive(Debug, Clone)]
pub struct ArbBlockAssembler<ChainSpec>(core::marker::PhantomData<ChainSpec>);

impl<ChainSpec> ArbBlockAssembler<ChainSpec> {
    pub fn new(_chain_spec: Arc<ChainSpec>) -> Self {
        Self(core::marker::PhantomData)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArbNextBlockEnvAttributes {
    pub timestamp: u64,
    pub suggested_fee_recipient: Address,
    pub prev_randao: B256,
    pub gas_limit: u64,
    pub withdrawals: Option<Withdrawals>,
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: Bytes,
    pub max_fee_per_gas: Option<U256>,
    pub blob_gas_price: Option<u128>,
}
