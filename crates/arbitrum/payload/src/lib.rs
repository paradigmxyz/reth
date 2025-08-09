#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types_engine::{ExecutionPayloadInputV2, ExecutionPayloadV3};
use reth_payload_primitives::ExecutionPayload;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArbPayloadV1 {
    pub fee_recipient: Address,
    pub prev_randao: B256,
    pub gas_limit: u64,
    pub base_fee_per_gas: U256,
    pub extra_data: Bytes,
    pub block_number: u64,
    pub timestamp: u64,
    pub gas_used: u64,
    pub transactions: Vec<Bytes>,
    pub withdrawals: Option<Vec<Withdrawal>>,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArbPayload {
    v1: ArbPayloadV1,
}

impl ArbPayload {
    pub fn new(v1: ArbPayloadV1) -> Self {
        ArbPayload { v1 }
    }
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArbSidecar {
    pub parent_beacon_block_root: Option<B256>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArbExecutionData {
    pub payload: ArbPayload,
    pub sidecar: ArbSidecar,
    pub parent_hash: B256,
    pub block_hash: B256,
}

impl ArbExecutionData {
    pub fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    pub fn v2(payload: ExecutionPayloadInputV2) -> Self {
        let ep = payload.execution_payload;
        let v1 = ArbPayloadV1 {
            fee_recipient: ep.fee_recipient,
            prev_randao: ep.prev_randao,
            gas_limit: ep.gas_limit,
            base_fee_per_gas: ep.base_fee_per_gas,
            extra_data: ep.extra_data.clone(),
            block_number: ep.block_number,
            timestamp: ep.timestamp,
            gas_used: ep.gas_used,
            transactions: ep.transactions.clone(),
            withdrawals: payload.withdrawals.clone(),
        };
        ArbExecutionData {
            payload: ArbPayload { v1 },
            sidecar: ArbSidecar { parent_beacon_block_root: None },
            parent_hash: ep.parent_hash,
            block_hash: ep.block_hash,
        }
    }

    pub fn v3(payload: ExecutionPayloadV3, parent_beacon_block_root: B256) -> Self {
        let ep = &payload.payload_inner.payload_inner;
        let v1 = ArbPayloadV1 {
            fee_recipient: ep.fee_recipient,
            prev_randao: ep.prev_randao,
            gas_limit: ep.gas_limit,
            base_fee_per_gas: ep.base_fee_per_gas,
            extra_data: ep.extra_data.clone(),
            block_number: ep.block_number,
            timestamp: payload.timestamp(),
            gas_used: ep.gas_used,
            transactions: ep.transactions.clone(),
            withdrawals: Some(payload.withdrawals().clone()),
        };
        ArbExecutionData {
            payload: ArbPayload { v1 },
            sidecar: ArbSidecar { parent_beacon_block_root: Some(parent_beacon_block_root) },
            parent_hash: ep.parent_hash,
            block_hash: ep.block_hash,
        }
    }

}

impl ExecutionPayload for ArbExecutionData {
    fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    fn block_hash(&self) -> B256 {
        self.block_hash
    }

    fn block_number(&self) -> u64 {
        self.payload.block_number()
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.payload.as_v1().withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.sidecar.parent_beacon_block_root
    }

    fn timestamp(&self) -> u64 {
        self.payload.timestamp()
    }

    fn gas_used(&self) -> u64 {
        self.payload.as_v1().gas_used
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
            gas_used: 0,
            transactions: txs.clone(),
            withdrawals: None,
        };
        assert_eq!(v1.block_number(), block_number);
        assert_eq!(v1.timestamp(), timestamp);
        assert_eq!(v1.transactions().len(), txs.len());

        let p = ArbPayload { v1: v1.clone() };
        assert_eq!(p.block_number(), block_number);
        assert_eq!(p.timestamp(), timestamp);
        assert_eq!(p.transactions().len(), txs.len());

        let ed = ArbExecutionData { payload: p.clone(), sidecar: ArbSidecar { parent_beacon_block_root: Some(prev_randao) }, parent_hash: B256::ZERO, block_hash: B256::ZERO };
        assert_eq!(ed.parent_hash(), B256::ZERO);
        assert_eq!(ed.payload.as_v1().fee_recipient, fee_recipient);
        assert_eq!(ed.payload.as_v1().gas_limit, gas_limit);
        assert_eq!(ed.payload.as_v1().base_fee_per_gas, base_fee_per_gas);
    }
}
