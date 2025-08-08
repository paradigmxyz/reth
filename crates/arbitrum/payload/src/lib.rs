#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::{bytes::Bytes, Address, B256, U256};

#[derive(Clone, Debug, Default)]
pub struct ArbPayloadV1 {
    pub fee_recipient: Address,
    pub prev_randao: B256,
    pub gas_limit: u64,
    pub base_fee_per_gas: U256,
    pub extra_data: Bytes,
    pub block_number: u64,
    pub timestamp: u64,
    pub transactions: Vec<Bytes>,
}

impl ArbPayloadV1 {
    pub fn block_number(&self) -> u64 {
        self.block_number
    }
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }
    pub fn transactions(&self) -> &Vec<Bytes> {
        &self.transactions
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArbPayload {
    v1: ArbPayloadV1,
}

impl ArbPayload {
    pub fn as_v1(&self) -> &ArbPayloadV1 {
        &self.v1
    }
    pub fn block_number(&self) -> u64 {
        self.v1.block_number
    }
    pub fn timestamp(&self) -> u64 {
        self.v1.timestamp
    }
    pub fn transactions(&self) -> &Vec<Bytes> {
        &self.v1.transactions
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArbSidecar {
    pub parent_beacon_block_root: Option<B256>,
}

#[derive(Clone, Debug, Default)]
pub struct ArbExecutionData {
    pub payload: ArbPayload,
    pub sidecar: ArbSidecar,
    pub parent_hash: B256,
}

impl ArbExecutionData {
    pub fn parent_hash(&self) -> B256 {
        self.parent_hash
    }
}
