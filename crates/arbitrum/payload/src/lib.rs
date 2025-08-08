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
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes::Bytes as ABytes, b256, Bytes};

    #[test]
    fn v1_accessors_return_expected_fields() {
        let fee_recipient = address!("00000000000000000000000000000000000000ff");
        let prev_randao = b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let base_fee_per_gas = U256::from(1000u64);
        let gas_limit = 30_000_000u64;
        let block_number = 12345u64;
        let timestamp = 1_700_000_000u64;
        let txs: Vec<Bytes> = vec![ABytes::from_static(b"\x01\x02").into(), ABytes::from_static(b"\x03").into()];

        let v1 = ArbPayloadV1 {
            fee_recipient,
            prev_randao,
            gas_limit,
            base_fee_per_gas,
            extra_data: Bytes::default(),
            block_number,
            timestamp,
            transactions: txs.clone(),
        };
        assert_eq!(v1.block_number(), block_number);
        assert_eq!(v1.timestamp(), timestamp);
        assert_eq!(v1.transactions().len(), txs.len());

        let p = ArbPayload { v1: v1.clone() };
        assert_eq!(p.block_number(), block_number);
        assert_eq!(p.timestamp(), timestamp);
        assert_eq!(p.transactions().len(), txs.len());

        let ed = ArbExecutionData { payload: p.clone(), sidecar: ArbSidecar { parent_beacon_block_root: Some(prev_randao) }, parent_hash: B256::ZERO };
        assert_eq!(ed.parent_hash(), B256::ZERO);
        assert_eq!(ed.payload.as_v1().fee_recipient, fee_recipient);
        assert_eq!(ed.payload.as_v1().gas_limit, gas_limit);
        assert_eq!(ed.payload.as_v1().base_fee_per_gas, base_fee_per_gas);
    }
}
